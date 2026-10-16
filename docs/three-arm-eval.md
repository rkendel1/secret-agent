# Three-arm evaluation: bare vs context-assisted vs AppPort-native

**Research question.** Does an agent built around AppPort's capability contracts and verification workflow achieve higher verified task completion, fewer retries, and lower total inference and execution cost than an agent that merely receives the same application knowledge as context?

<!-- RESULTS -->

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
