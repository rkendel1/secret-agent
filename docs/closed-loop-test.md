# Closed-loop autonomy test

PR 3 points the agent from PR 2 at a real repository it was not written for,
[`rkendel1/appport-services`](https://github.com/rkendel1/appport-services)
(pinned at `81426d6`), and asks three questions:

1. Is this a real autonomous agent?
2. Does AppPort knowledge actually help?
3. Is it a decent coding agent when the knowledge is present?

```sh
scripts/closed-loop-test.sh                                    # deterministic stages; what CI runs
scripts/closed-loop-test.sh --config examples/models/ollama.json     # + evaluation with a local model
scripts/closed-loop-test.sh --config examples/models/anthropic.json  # + evaluation with a remote model
```

## What happened on appport-services

`appport-services` is an Express service library (`@appport/services`). It
depends on `@appport/protocol` for its UI contract only; it does not run an
AppPort application runtime and has no `customers.lookup` capability.

| Stage | Project state | What the agent did | Model calls |
|---|---|---|---|
| 1 | as published | **refused**: `appport.runtime-absent`, `application.missing` — nothing publishes a manifest, so there is nothing to observe ([log](closed-loop/1-services-clone.txt)) | 0 |
| 2 | after `npx appport@1.1.20 init` + `@appport/sdk` | observed the new `appport/server.ts` application, its three capabilities and their inline handlers, read the contract from the running runtime, then **refused**: `contract.unverifiable` — no capability declares an effect, nothing declares verification, so no behaviour of `documents.open@1` could be checked ([log](closed-loop/2-services-init.txt)) | 0 |
| 3 | + the contract declarations in [`declare-contract.patch`](../examples/appport-services/declare-contract.patch) | planned `replace documents.open@1`, rejected the first proposal, rolled it back, accepted the retry ([progress](closed-loop/3-services.log.txt), [report](closed-loop/3-services.report.json), [every step](closed-loop/3-services.run.jsonl)) | 2 |

Stage 2 is a result in its own right. The contract `appport init` generates
says what each capability takes and returns, but not whether it changes state
or how a change could be confirmed. An agent that accepted a change there would
be accepting it on structure alone, so this one refuses — before any model call
— and says what to declare. The patch for stage 3 declares exactly that, the
way the project's author would: `effect` and `idempotency` on each capability,
a semantic concept for document ids, `NOT_FOUND` as `documents.open`'s failure,
the document resource and its access, and how a save is verified (re-read it
through `documents.open@1`; corroborate with `documents.changed`).

### Stage 3 in detail

| | |
|---|---|
| Intent | `replace documents.open@1: return a copy of the stored document instead of the live record` |
| Observed | 131 source files; entry `appport/server.ts#application`; 3 capabilities located (handlers inline in the contract module) |
| Contract (from the runtime) | 6 capability versions, graph 9 nodes / 12 edges, `contractDigest sha256:1142b613…` |
| Decided without a model | 12 decisions, 8 replacement constraints, 8 acceptance checks; `documents.save@1` depends on the target through its declared state verification; `documents.list@1` enumerates existing documents; the session gets only `documents.read`, `documents.write` |
| Context sent | 1 of 131 files (4,391 of 1,042,104 source bytes) + 918 bytes of contract |
| Attempt 1 | the proposal returned a copy **and** added `openedAt` to the output schema and the result — self-consistent, so the runtime served it. **Rejected**: `contract.identity` (`contractDigest` and the `documents.open@1` pin changed) and `behavior.regression` (`doc_welcome` now answers with an extra field). Rolled back. |
| Attempt 2 | given those violations: **accepted**; all 8 checks pass; `documents.save@1` verified (`document-saved` confirmed, `changed-event` confirmed) |
| Totals | 2 model calls, 4,202 in / 2,322 out tokens (estimated), 1 rejection, 1 rollback, 4 application runs, 28 AppPort envelopes, 6 verification methods evaluated, 4.2 s wall-clock |

Because `appport init` puts handlers inline in the contract module, the only
possible edit boundary is the file that also holds the contract. The contract
digest is then the only thing between a model and a silent contract change —
attempt 1 is that case, and the check caught it.

The same assertions run on `examples/customers-app` (`replace customers.lookup@1`):
accepted after 1 rejection and 1 rollback (`failures.declared`,
`behavior.regression`), 2 model calls, 0.9 s ([report](demo/replace-customer-lookup.report.json)).

## With a real model (PR 4)

The loop has since been run with `claude-opus-5-5` — 41 trials, no scripted completions; see
[`eval-report.md`](eval-report.md). Summary:

| project | arm | accepted on 1st attempt | accepted after retry | rejected / error |
|---|---|---|---|---|
| appport-services (`documents.open@1`) | knowledge | 10/10 | 0 | 0 |
| appport-services | bare | 10/10 | 0 | 0 |
| customers-app (`customers.lookup@1`) | knowledge | 10/10 | 0 | 0 |
| customers-app | bare | 0/10 | 0 | 9 rejected, 1 refusal |

Without the graph-selected context the model never sees `src/store.mjs`, guesses a module that does not exist,
and the application fails to load; retries with the violations do not recover it. With it, every proposal passes
all 8 checks on the first try. Where the task needs nothing outside the editable file, both arms succeed and the
knowledge costs ~31% more tokens.

The sections below describe the scripted runs PR 3 shipped; they remain the deterministic CI test.

## The model in these runs

Both accepted runs use `--provider scripted`: pre-written completions
(`examples/*/scripted/`), replayed in order. They stand in for a model so the
loop is reproducible in CI; each first completion is a plausible mistake
written to exercise rejection, rollback and retry. **They are not model output,
and these runs say nothing about how often a real model succeeds** — for that, see
[`eval-report.md`](eval-report.md).

## Answers, as far as the evidence goes

**1. Is it a real autonomous agent?** On the evidence here: the loop is real
and closed. Pointed at a repository it was not built for, it found the
application, read the contract from the running runtime, worked out what the
change touches, decided what to accept, ran the application, rejected a
contract-changing proposal on evidence the runtime did not produce, restored
the project, retried with the violations, and accepted only a change every
check passed — with no human step, and with the model used once per attempt and
nowhere else. It also refused, twice, where proceeding would have been
guessing. What is not yet shown is that a real model gets through the loop; the
scripted completions cannot show that.

**2. Does AppPort knowledge help?** Shown: the knowledge is what makes the
loop possible at all. Without a published contract the agent cannot start
(stage 1); with a contract that declares no effects or verification it cannot
verify (stage 2); every check that rejected a proposal (contract digest and
pin, declared failures, declared verification, concept-keyed regression
probes) comes from the contract. Not yet shown: whether the knowledge in the
model's *context* raises the success rate. `appport-agent eval` measures exactly
that — the same intent, N trials, `knowledge` vs `bare` context, the same
verifier — and needs a real model.

**3. Is it a decent coding agent with the knowledge present?** Not measurable
without a real model. The eval reports acceptance rate, first-attempt rate,
attempts to accept, tokens, rejections by check, rollbacks and wall time per
arm.

## The evaluation

```sh
appport-agent eval --project <dir> --intent "<intent>" --config <model.json> \
  --trials 5 --arms knowledge,bare --out target/eval
```

- each trial is a fresh copy of the project; the template is never modified;
- `knowledge`: the agent's task (contract slice, constraints, checks, graph-selected files);
- `bare`: what a plain chat-completion wrapper sends — the goal and the editable file;
- both arms get the verifier's violations on retry, so the difference is what the
  model is told up front; the first-attempt rate isolates that most cleanly;
- output: `eval.json`, `eval.md`, and every trial's report and run log.

On `customers-app` the arms differ sharply: `bare` does not see `store.mjs`, so
the model has to guess the store's API. On `appport-services` the editable file
is the contract module, so `bare` sees the contract in source but not the
constraints, the checks or the verification that depends on the target.

To get the numbers, either run `scripts/closed-loop-test.sh --config …` where a
model is reachable, or run the manual **agent-live** workflow (local leg: Ollama
on the runner; remote leg: needs the `ANTHROPIC_API_KEY` repository secret).
That workflow is written but has not been run yet.

## CI

`agent-e2e` runs the ignored end-to-end tests (including the eval harness) and
`scripts/closed-loop-test.sh`, which fails unless stages 1 and 2 refuse with the
expected codes and stages 3 and 4 each end in a verified, accepted change after
at least one rejection and one rollback (`scripts/check-report.mjs`).
