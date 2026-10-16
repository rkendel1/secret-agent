# Coding-agent evaluation (no AppPort)

Does the agent work as a coding agent when AppPort is not involved at all?

This path never touches AppPort knowledge, contracts or verification methods
(`agent_core::generic`, `appport_agent::coding`). The only sources of truth are:
- the repository itself;
- the project's own test command;
- acceptance tests the agent never sees (`hidden/`), run after it finishes.

## The loop

1. **Observe**: file tree and import graph; run the project's test command once as a baseline.
2. **Plan**:
   - **Editable**: the modules the failing tests (named in the baseline output) import.
   - **Read-only context**: files whose names match the intent. They only bound the change when no failing test points anywhere.
   - **Tests**: never editable.
   - **Refusal**: when nothing points at any file, the agent refuses rather than guesses.
3. **Propose**: ask the model for the change. It is never asked to judge it.
4. **Check boundary**: a change to a test or to a file outside the boundary is rejected without being written.
5. **Verify**: apply the change and run the test command. Accept only on exit 0. Otherwise roll back, and retry with the tail of the test output (up to 3 attempts).
6. **Score** (evaluation only): copy the hidden acceptance tests in and run them. Success means the change was accepted **and** passes them, so a change that only satisfies the visible tests counts as a false accept.

## Fixtures

`fixtures/coding/` holds two ordinary Node projects. Each has:
- decoy modules;
- a failing visible test;
- hidden acceptance tests;
- a golden solution;
- the files a person would change.

`scripts/check-coding-fixtures.sh` checks that each fixture is sound: it fails at the start, and the golden solution passes both test sets.

| task | kind | what it takes |
|---|---|---|
| `bugfix-invoice-discount` | bug fix | a percentage discount is applied as a fraction, in one of 4 modules |
| `feature-slug-max-length` | feature | `slugify(text, { maxLength })`: word-boundary truncation, no trailing hyphen, cut a word only when unavoidable |

Two arms, judged identically:
- **`observed`**: the editable file, the failing test, what the code imports, and the baseline test output.
- **`minimal`**: the intent and the editable file only, with nothing observed about it.

## Results (Opus 5.5, 2026-10-16)

`claude-opus-5-5`, Anthropic API, `effort: high`, 10 trials per task and arm, up to 3 attempts each.
Total cost: $0.79 (input $4, output $20 per million tokens).
Raw runs are in [`docs/coding-eval/`](coding-eval/): every prompt, response, test run, accepted diff and hidden-test output.

| task | arm | success | 1st attempt | false accepts | rejected | boundary / test-edit attempts | stayed in intended files | tokens in / out | mean wall |
|---|---|---|---|---|---|---|---|---|---|
| bugfix-invoice-discount | observed | **10/10** | 10 | 0 | 0 | 0 / 0 | 10/10 | 21,709 / 5,280 | 5.7 s |
| bugfix-invoice-discount | minimal | **10/10** | 10 | 0 | 0 | 0 / 0 | 10/10 | 5,610 / 3,906 | 5.8 s |
| feature-slug-max-length | observed | **10/10** | 10 | 0 | 0 | 0 / 0 | 10/10 | 24,940 / 9,188 | 9.3 s |
| feature-slug-max-length | minimal | **10/10** | 10 | 0 | 0 | 0 / 0 | 10/10 | 4,420 / 9,575 | 9.8 s |

The agent ran the project's test command in every trial: twice each, once at baseline and once on the proposal.

## What this shows

- **The general loop works on ordinary repositories:**
  - 40/40 verified successes, each judged by the project's own tests and confirmed by tests the agent never saw.
  - No false accepts.
  - No attempt to edit tests or leave the boundary.
- **The observed context did not matter here.** Both tasks are small, and everything needed is in the one editable file. The minimal arm cost about 35% less and was no slower. This is a ceiling result: it says nothing about tasks where the needed facts live elsewhere.

## What it does not show

- **Harder tasks:** multi-file changes, finding an existing helper, refactors that must keep behavior, and under-specified requests. These are the next fixtures; the format already supports them.
- **Other models:** one model only.
- **Other ecosystems:** one ecosystem (Node). The observer's import graph is JavaScript/TypeScript-only, so other languages rely on failing-test names and intent matching.
- **The AppPort comparison is not like for like.** The AppPort result (`docs/eval-report.md`) was 10/10 with knowledge vs 0/10 without on a task whose missing fact lived in another file. That task tests something these fixtures do not.

## Reproduce

```sh
scripts/check-coding-fixtures.sh
appport-agent code-eval --tasks fixtures/coding --config examples/models/anthropic.json \
  --trials 10 --arms observed,minimal --out target/coding-live --require-test-runs
```

Or run the manual **agent-live** workflow with `legs: coding` (Vercel AI Gateway, `min_success` threshold).
