# Architecture decisions

Each decision gives the choice, the alternatives considered, and the evidence. "Revisit when" names what would change it.

## D1. Strategies live in `appport-agent`, not a new crate

- **Chosen:** `crates/appport-agent/src/strategy.rs` and `experiment.rs`.
- **Alternative:** an `agent-strategy` crate.
- **Why:** both modules depend on the loop (`coding.rs`), the host (`host.rs`, for `appport-aware`) and the report types. A separate crate would need those moved or duplicated. The deterministic parts strategies build on (`observe`, `plan_coding`, the knowledge layer) were already in `agent-core`. The only change there is the prompt layout split (`TaskView` / `render_coding_task`), which is pure.
- **Revisit when:** a second binary needs the strategies without the CLI.

## D2. Interfaces only where there are rival implementations to compare

- **`ContextStrategy` (4 implementations)** and **`RecoveryPolicy` (3)** are traits. Each implementation has an id and a version, recorded in every report.
- **Planning and verification are not traits.** They are the authorities every arm shares. Making them pluggable would let an experiment change what "success" means.
- **Editing is not a trait yet.** It has one implementation (whole-file blocks into `edit::apply`). A second format would be a parse step in front of `edit::apply`, which remains the only writer.
- **Execution policy is not a trait.** It is the boundary, not a variable.
- **Alternative:** a trait per primitive (7 traits). Rejected, because most would have one implementation and no experiment, which is indirection with nothing to measure. Goose shows the shape a richer recovery substrate could take (`Operation`/`StepResult`, `crates/goose-agent/src/operation.rs`) once there are policies that need multi-step state.

## D3. A strategy proposes; the boundary decides; the verifier judges

`RecoveryDecision` has three variants:
- retry with feedback;
- retry with extra **read-only** files;
- stop.

None can name an editable file, a permission or an acceptance command.

The loop enforces this twice:
- requested files are filtered against the planner's editable set;
- `edit::apply` checks the boundary on every write.

A strategy cannot mark an attempt as accepted: acceptance is the test command's exit code, and in evaluation the hidden tests.

- **Evidence:** every reference agent that lets the model or a model judge declare success was rejected on that primitive (see the matrix, *Verification*).
- **Test:** `tests/experiments.rs`, `recovery_policies_are_recorded_and_never_widen_the_boundary`.

## D4. The baseline is the old behaviour, byte for byte

- **What the defaults are:** `observed` + `feedback` with the planner's 48 KB budget.
- **Proof that they match:** they render exactly the prompts `coding_task` rendered (`tests/experiments.rs`, `the_baseline_strategies_reproduce_the_existing_prompts_byte_for_byte`). `code-eval` without any new flag runs the same arms as before. So results from before and after this change stay comparable.

## D5. One evaluation harness, extended

- **Chosen:** `coding::coding_eval` (behind `code-eval`) gained everything an experiment needs. The `experiment` command only resolves a specification into its options:
  - named arms;
  - balanced order;
  - limits;
  - the free-gateway preflight;
  - provenance;
  - fixture digests;
  - categories;
  - comparisons.
- **Alternative:** a new benchmark runner. Rejected, because there would be two scoring paths that could drift.
- `arm-eval` (the three-arm AppPort study) stays separate, because its arms differ in loop structure, not in strategy. Its limits, provenance and preflight code is shared (`arms::provenance_of`, `digest_files`, `price`).

## D6. Experiment specifications reuse model configuration files

- **The format:** `secret-agent.experiment/1` names a model configuration file (`examples/models/*.json`), plus optional `overrides` of the same fields.
- **Validation:** overrides are checked by the same `deny_unknown_fields` deserializer, so there is no second configuration system.
- **Unknown input is an error:** unknown fields, strategies and schema versions.

## D7. Free gateway models only, enforced in the harness

- **Gateway runs are always preflighted:**
  - any evaluation whose model is at `https://ai-gateway.vercel.sh/v1` runs the free-model preflight first, whether or not the specification asks for it;
  - a failed preflight runs no trial;
  - the model must be on the allowlist and priced at zero by the live catalog.
- **No Anthropic:** an experiment specification using the `anthropic` provider is refused. Local OpenAI-compatible servers and scripted completions are allowed.
- **Shared rules:** the preflight, allowlist and redaction are the existing `agent_model::gateway` code.

## D8. Failures are classified, never scored against a strategy

**Categories:**
- `verified-success`
- `accepted-unverified` (visible tests passed, hidden failed)
- `rejected`
- `timed-out`
- `patch-failed` (no reply could be applied)
- `limitation`
- `provider-error`
- `infrastructure-error`
- `not-run`

**Rules:**
- **The success rate's denominator** excludes provider and infrastructure failures. They say nothing about a strategy, and they are counted and shown.
- **No success without evidence:** an accepted trial with missing required evidence (hidden tests declared but not run) is an `infrastructure-error`, never a success.
- **Precedent:** Pi's blocked-pair accounting (`packages/evals/src/report.ts`) is the reference for withholding a result rather than dropping failures. Gemini CLI's "skip on API error" is the counterexample.

## D9. No LLM summarization of context

Each attempt's context is rebuilt deterministically from the selection and the latest feedback. LLM summaries (Aider, OpenHands condensers, Gemini compression) would make the context itself model output. Deterministic truncation ideas (head and tail, shrink ladders) are candidates instead.

## D10. Dependency facts are lexical for now, and say so

- **What `dependency-aware` reports:** importers from the observed import graph, and whole-word uses of an editable file's exported names in those importers.
- **Labelled as lexical:** every gap is recorded as an unknown, such as aliases, dynamic calls, transitive users, default exports and re-exports. No edge is invented.
- **Alternative:** tree-sitter now (Aider `repomap.py`, Goose `analyze/`). Deferred, because it adds grammar dependencies, and the lexical version already lets the experiment ask whether call-site facts help at all.
- **Revisit when:** `dependency-aware` shows a gain, or misses on a task where the facts were resolvable.

## D11. `appport-aware` reads the contract from the running application

- **The source of facts:** the application's own `appport.manifest@1`, read through the existing host with no permissions, and the existing knowledge layer (`inspect`, `replacement_boundary`). No second contract authority.
- **The source of the mapping:** source observation maps capabilities to the editable handler files.
- **When it adds nothing:** when the repository is not an AppPort application, or the runtime cannot start, it records that as an unknown and adds nothing.
- **Fixture:** `fixtures/coding/appport-readback` was added so that this arm has a task where it can matter.

## D12. Comparisons are descriptive

The report gives deltas against the baseline (success rate, calls, tokens, median wall time, context bytes), and the component an arm changes. It computes no significance test. With fixtures this small, one would be misleading.

**Revisit when:** suites and trial counts support paired tests. Pi's flaky/saturated flags are the next step.

## Future experiment candidates (not built)

| candidate | source | blocked on / prerequisite |
|---|---|---|
| tree-sitter repo map / call graph as a context strategy | Aider `repomap.py`; Goose `analyze/` | grammar dependencies; D10 evidence |
| one-warning-then-stop recovery | OpenHands stuck detector; Gemini loop detection | none: a new `RecoveryPolicy` |
| fresh-reset retry (no history of the failed attempt) | SWE-agent `RetryAgent` + `hard_reset`; Goose recipe retry | none: a new `RecoveryPolicy` |
| bounded test-output digest | OpenHands `truncate.py`; OpenCode shrink ladder | none: a parameter of the feedback text |
| pre-test gates on whole-file output (placeholders, truncation) | Gemini CLI omission detector; Pi length-truncation rule | an editing-stage check before `edit::apply` |
| search/replace edit format | Pi `edit`, SWE-agent `str_replace` | a second edit parser in front of `edit::apply` |
| containers per trial; OS sandbox | SWE-agent, OpenHands benchmarks, Gemini sandboxes | rkendel1/secret-agent#10 |
| pass@k / pass^k, flaky and saturated flags | Cline evals; Pi evals | more trials per cell |
| multi-agent orchestration, browser automation, persistent memory | various | **non-goals** for now: no measurement shows a need |
