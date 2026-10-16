# Real-model evaluation: does AppPort knowledge help?

The closed loop from PR 2/3, driven by a real model for the first time.

- **Model**: `claude-opus-5-5` (Anthropic API, adaptive thinking, `effort: high`, `max_tokens` 16000), via
  [`examples/models/anthropic.json`](../examples/models/anthropic.json). No scripted completions anywhere below.
- **Run**: 2026-10-15, 41 trials, 58 model calls, **$1.62** of API spend (input $4 / output $20 per MTok,
  thinking tokens included in output).
- **Scoring**: the deterministic verifier only — the same 8 checks for every proposal, from the contract the
  application's runtime publishes and from what the running application answers. No human and no model judges
  any outcome.
- **Raw evidence**: [`docs/eval/`](eval/) — per trial, the exact prompt sent, the model's full response, the
  evidence collected from the running application, the verdict (`run.jsonl`), the report (`report.json`) and,
  when accepted, the change (`accepted.diff`).

## The two arms

| arm | what the model is told |
|---|---|
| `knowledge` | the agent's task: the capability's contract slice (`CapabilityView`), the replacement constraints, the acceptance checks, and the files the integration graph selects (the target's implementation, what it imports, and the implementations that read or change the same declared resource) |
| `bare` | the goal and the editable file — what a plain chat-completion wrapper would send |

Both arms get up to 3 attempts, and both receive the verifier's concrete violations on retry. Each trial runs on
a fresh copy of the project.

## Results

### 1. appport-services — `replace documents.open@1: return a copy of the stored document instead of the live record`

Project: [`rkendel1/appport-services`](https://github.com/rkendel1/appport-services) at `81426d6`, after
`npx appport@1.1.20 init` and the contract declarations in
[`declare-contract.patch`](../examples/appport-services/declare-contract.patch). The handler lives in the same
file as the contract, so the editable file is the contract module.

| arm | trials | accepted on 1st attempt | accepted after retry | rejected | errors |
|---|---|---|---|---|---|
| knowledge | 10 | **10** | 0 | 0 | 0 |
| bare | 10 | **10** | 0 | 0 | 0 |

| arm | model calls | tokens in / out | tokens per accepted | contract edits | boundary violations | mean wall time |
|---|---|---|---|---|---|---|
| knowledge | 10 | 29,040 / 17,347 | 4,639 | 0 | 0 | 16.1 s |
| bare | 10 | 18,190 / 17,207 | 3,540 | 0 | 0 | 16.3 s |

Every accepted change was a one-line handler edit (three equivalent forms across the 20 trials:
destructure-and-return, explicit field copy, spread). No proposal touched the contract, even though the whole
contract module was editable. The task is within reach without the knowledge; with it, the same result cost
~31% more tokens (the contract slice and checks in the prompt).

### 2. customers-app — `replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export`

Project: [`examples/customers-app`](../examples/customers-app) (built with `@appport/core` 1.2.0).

| arm | trials | accepted on 1st attempt | accepted after retry | rejected (all 3 attempts) | errors |
|---|---|---|---|---|---|
| knowledge | 10 | **10** | 0 | 0 | 0 |
| bare | 10 | **0** | 0 | 9 | 1 (refusal) |

| arm | model calls | tokens in / out | tokens per accepted | rejected attempts | rollbacks | unusable responses | mean wall time |
|---|---|---|---|---|---|---|---|
| knowledge | 10 | 26,930 / 4,176 | 3,111 | 0 | 0 | 0 | 5.3 s |
| bare | 27 | 23,128 / 20,605 | – (none accepted) | 27 | 19 | 8 | 23.9 s |

Why `bare` failed, from the run logs:

- **Attempt 1, every trial**: the model guessed a store module, `../store/customers.mjs`, which does not exist
  (the store is `src/store.mjs`, exporting `getCustomer`). Node failed to load the application
  (`ERR_MODULE_NOT_FOUND`), so `runtime.load` and every check that needs a running application failed.
- **Declining to guess, 8 responses** (7 at attempt 2, 1 at attempt 3): given that violation, the model said
  *"I can't make this change correctly without information I don't have, so I'm not sending a file"* — recorded
  as an unusable response. That is the right call for the model; it is still not a change.
- **Guessing again**: the remaining attempts imported another name from the same nonexistent module and failed
  the same way. One attempt instead fell back to the legacy CRM export: the application ran and every other check
  passed, but `customers.register@1`'s declared verification stayed unverifiable — a customer just registered
  still could not be read back, which is the defect the change was meant to fix.
- **Trial 5**: the request was declined by a safety classifier (`stop_reason: refusal`, category `cyber`) — a
  false positive on an ordinary refactor. The agent records it as an error and does not re-run it on another
  model, so the measurement stays one model.

The `knowledge` arm was given `src/store.mjs` because the integration graph shows `customers.register@1`
*changes* `urn:example:customers:customer`, which `customers.lookup@1` *reads* — and every one of its 10
proposals imported `getCustomer` from `../store.mjs`, kept `NOT_FOUND` as the declared failure, and passed all 8
checks on the first attempt, at 1/5 of the bare arm's output tokens and wall time.

10/10 against 0/10 (9 rejected, 1 refusal): Fisher's exact test p < 0.0001.

### The single run that opened the evaluation

Before the comparison: one `knowledge` trial on appport-services, accepted on the first attempt — 1 model call,
2,904 / 1,733 tokens, 16 s ([`docs/eval/single`](eval/single)).

## Answers

**1. Can a real model finish the loop?** Yes. 21 of 21 `knowledge` trials across both projects ended in a
verified, accepted change on the first attempt, with no human step. Every structural decision and every
acceptance was made by the deterministic core; the model was called once per attempt and for nothing else.

**2. Does AppPort knowledge in the prompt improve success or reduce cost?** It depends on whether the task needs
anything outside the editable file.

- When it does (customers-app: the store the new implementation must read lives elsewhere), the knowledge is the
  difference between 10/10 and 0/10. The graph-selected context is what supplied the missing fact; retries with
  precise violations could not, because the violations say what went wrong, not where the store is.
- When it does not (appport-services: everything is in the one file), both arms succeed every time and the
  knowledge only costs tokens (+31%).
- What these two arms do not separate: how much of the `customers-app` effect comes from the graph-selected
  *files* versus the contract slice, constraints and checks in the prompt. Both come from the knowledge layer,
  but a third arm (graph-selected files without the contract text) would split them. That arm is not run here.

**3. Is it a usable coding agent on this class of task?** On bounded, contract-preserving replacements of one
capability in an AppPort application that declares its effects and verification: yes — 100% verified success at
about 5–16 s and $0.02–0.05 per change with Opus 5.5. The deterministic guards were not exercised by a real
model in these runs: no real proposal edited the contract or left the boundary (both guards were exercised in PR
3 with scripted proposals and remain in place). The limits are the ones the loop states: one capability, one
editable file, and a contract that declares enough to verify (PR 3, stage 2 refuses otherwise).

## Caveats

- Two tasks, one model, 10 trials per arm. The tasks are small; the result says nothing yet about larger or
  multi-file changes.
- `bare` is a deliberately minimal baseline: it has no tools to explore the repository. A tool-using coding agent
  could find `store.mjs` by searching. What the comparison shows is that the knowledge layer delivers the right
  context *without* exploration, and that a verifier alone cannot make up for missing context.
- One `bare` trial was lost to a safety-classifier refusal; counting it as a failure or excluding it does not
  change the result.

## Reproduce

```sh
export ANTHROPIC_API_KEY=...          # and ANTHROPIC_WORKSPACE_ID=wrkspc_... for a key not scoped to a workspace
TRIALS=10 scripts/closed-loop-test.sh --config examples/models/anthropic.json
# results: target/closed-loop/eval-appport-services/eval.md, target/closed-loop/eval-customers-app/eval.md
```

The manual **agent-live** workflow no longer calls Anthropic's API: it runs only a free Vercel AI Gateway model, after a preflight (`legs: gateway`; see [the three-arm evaluation](three-arm-eval.md#live-runs-free-vercel-ai-gateway-model-only)). The results above were produced locally with the Anthropic configuration shown.
Expect different token counts and wording from run to run; the verifier's decisions are what to compare.
