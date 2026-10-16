# Three-arm evaluation: bare vs context-assisted vs AppPort-native

**Research question.** Does an agent built around AppPort's capability contracts and verification workflow achieve higher verified task completion, fewer retries, and lower total inference and execution cost than an agent that merely receives the same application knowledge as context?

## Results (Opus 5.5, 2026-10-16)

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

| arm | oracle outcome | what the proposals actually did |
|---|---|---|
| A — bare | verified 3/5 | kept calling the legacy `lookupCustomer` |
| B — context | verified 5/5 | 4 read the legacy CRM export directly, 1 kept the lookup; none used the store (the graph gave B no path to `store.mjs`, because billing declares no resource access) |
| C — native | wrong limitation 5/5 | declined: `billing.contact@1` declares `requires: customers.lookup@1` (`required: true`), so bypassing the lookup makes a declared dependency false, which is a contract change; and the capability declares no verification |

**No arm achieved the stated goal, and the oracle counted 8 trials as successes anyway.**
- **Why the oracle passed them:** it verifies preservation (same answers, same contract, declared verification), not goal achievement. For `replace-lookup` the goal is observable, because `customers.register@1`'s declared verification only passes when the lookup reads the store. For `billing-from-store` nothing observable distinguishes "reads the store" from "unchanged".
- **C's reading is defensible:** the task asks for something the contract's declared dependency contradicts.

This task is excluded from the headline and reported here as the raw outcome. Fixing it needs either a goal check the oracle can observe, or a contract whose declared dependencies allow the change.

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
- **The oracle:** it measures preservation and declared verification. It needs a goal check wherever the goal is not observable through declared verification; `billing-from-store` shows why.
- **A hybrid:** B's deterministic planning with C's limitation channel and discovery fallback is untested. The per-task numbers suggest it would keep most of C's coverage at close to B's cost.
- **Smaller or local models:** not tested.
- **The GitHub `agent-live` run of 2026-10-16:** it produced no measurements. Every model call was refused by the Vercel AI Gateway (`HTTP 403: Free tier users do not have access to this model`). The workflow now stops at a one-token preflight call when the key cannot use the model, and `arm-eval` fails when no model call succeeds.


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
- **The oracle is independent of every arm.** After a trial it:
  - re-plans the task's `oracleTarget` on a pristine copy;
  - collects a fresh baseline;
  - runs the final project;
  - judges it, with the changed files computed by diffing against the fixture.

  For a negative task, the project must be byte-identical to the fixture. No arm's own claim is credited.
- **Every trial is recorded** (`trial.json`, the arm's report and run log, `oracle.json`), including failures and budget-skipped trials.

### Unavoidable differences, recorded

- **A and B need a named target.** Their loop takes `replace <capability>@<version>`. On the two tasks without a target, they stop before any model call. This is what those workflows are, not a handicap added for the test.
- **Only C can report a limitation.** A and B have no channel for it. On a negative task they are safe only by failing closed: no proposal passes, so nothing is kept.
- **C chooses what to look at,** so it makes more model calls. Each reply is one tool call, so C's model calls include discovery and inspection. Those calls, their tokens and the bytes of AppPort results sent to the model are all counted.
- **The budgets differ in shape.** C's conversation is capped at 12 model calls. A and B make at most 3 calls, one per proposal.

## Task suite (`fixtures/appport-tasks/`)

| task | expect | category | target given | project |
|---|---|---|---|---|
| `replace-lookup` | verified change | contract-conforming change (the PR 4 task) | `customers.lookup@1` | `examples/customers-app` |
| `discover-readback` | verified change | capability discovery and selection | none | `examples/customers-app` |
| `billing-from-store` | verified change | incremental legacy replacement preserving behaviour | `billing.contact@1` | `examples/customers-app` |
| `contract-change` | limitation | unsupported: the goal needs an output-contract change | `customers.lookup@1` | `examples/customers-app` |
| `missing-capability` | limitation | missing capability (delete a customer) | none | `examples/customers-app` |
| `unverifiable-contract` | limitation | verification cannot establish the outcome | `customers.lookup@1` | `fixtures/appport-projects/customers-undeclared` (a contract with no effects or verification) |

No task needs GnuCOBOL, a network download or an external runtime beyond Node.js and the npm packages already used.

### Outcome categories

| category | meaning |
|---|---|
| `verified-success` | positive task; the arm claimed success and the oracle verified it |
| `false-success` | the arm claimed success, but the oracle did not verify it; or, on a negative task, the arm claimed success or left a change |
| `wrong-limitation` | positive task; the arm stopped with a limitation |
| `unverified` | positive task; no proposal passed |
| `correct-limitation` | negative task; the arm reported a limitation and the project is unchanged |
| `fail-closed` | negative task; no proposal passed and the project is unchanged, but nothing was reported |
| `provider-refusal`, `provider-error`, `harness-error` | not agent failures; counted separately |
| `not-run` | skipped because the budget was reached; listed, never dropped |

## Reproduce

```sh
npm ci --prefix examples/customers-app
# deterministic, no paid inference (what CI runs)
cargo test -p appport-agent --test arms -- --ignored
# live, budgeted
appport-agent arm-eval --tasks fixtures/appport-tasks --config examples/models/anthropic.json \
  --arms A,B,C --trials 5 --budget-usd 10 --out target/arms-live
```

Or run the manual **agent-live** workflow with `legs: arms`. It goes through the Vercel AI Gateway, with `budget_usd` as the cap.
