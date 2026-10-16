# Production readiness: can secret-agent be your primary coding agent?

## 1. Verdict

**Not ready for primary coding-agent use.**

**Conditionally ready for one bounded workflow:** a supervised `appport-agent run` that replaces the implementation of one AppPort capability in a JavaScript/TypeScript project kept in git, with evidence that the goal was achieved, on a machine where running model-written code is acceptable.

Why not primary use:

1. **Scope.**
   - The only command that changes your project is `run`, and its only intent is `replace <capability>@<version>`.
   - The general coding loop exists, but only `code-eval` uses it, and only on copies.
   - Most everyday coding tasks cannot be given to the agent. See RA-12, [#11](https://github.com/rkendel1/secret-agent/issues/11).
2. **No sandbox.** Model-written code runs with your filesystem and network access while it is verified. It no longer sees provider credentials, but it can see everything else. Unattended use and untrusted repositories are blocked by RA-11, [#10](https://github.com/rkendel1/secret-agent/issues/10).
3. **No live-model evidence for the current configuration.**
   - The free Gateway model has not been run end to end. This sandbox has no `AI_GATEWAY_API_KEY`, and this session cannot dispatch workflows.
   - The only real-model results (`docs/eval-report.md`, `docs/three-arm-eval.md`) are for Opus 5.5 on these small fixtures.

What *is* established, by deterministic tests that need no model or network: the safety and verification paths of `run`. Before this PR, four of them could fail in ways that destroy work or claim false success (P0, below). Each is now fixed and has a regression test that runs in CI.

## 2. Scope and commit reviewed

- **Base:** `main` at `8bf8e02` ("Merge pull request #9").
- **Code reviewed:**
  - `crates/agent-core`: observation, planning, the knowledge layer, conformance, the judge.
  - `crates/agent-model`: every provider, the gateway preflight.
  - `crates/appport-agent`: `run`, `edit`, `host`, `collect`, `native`, `coding`, `arms`, `eval`, the CLI.
  - `examples/customers-app`, `scripts/`, `.github/workflows/`.
  - Docs: `docs/agent-loop.md`, `docs/appport-knowledge.md`, `docs/three-arm-eval.md`, and the recorded trial artifacts.
- **Not reviewed:**
  - the AppPort packages themselves (`@appport/*`). Their surface is checked by the drift and parity tests.
  - the scripted fixture repositories' own code.
- **Base results:**
  - `cargo fmt --check` and `cargo clippy --all-targets -D warnings`: clean.
  - `cargo test --all -- --include-ignored`: **114 passed, 0 failed**.
  - `scripts/closed-loop-test.sh`: passed.
  - There were no pre-existing failures. Every failure listed below was found by review and reproduced, not by an existing test.

## 3. The execution path, as implemented

```
intent ─► observe ─► discover ─► plan ─► [lock, uncommitted-work check, baseline, goal evidence] ─► model ─► parse ─► boundary ─► apply ─► run app ─► judge ─► goal check ─► accept │ rollback ─► report
          (no exec)  (app, no    (no                                                              (only      (only      (lexical  (checked,  (no        (determ-  (fresh       (kept)  (checked)
                     permissions) model)                                                           here)      text)      + links)  atomic)    creds)     inistic)  process)
```

| transition | implementation | on failure | tests and evidence |
|---|---|---|---|
| intent | `agent_core::plan::Intent::parse` | `unsupported intent` → `limitation` (exit 3) | `agent-core/tests/agent.rs` `refuses_what_it_cannot_ground` |
| observe | `agent_core::observe::observe`: static scan, no execution; symlinks are reported, never followed (RA-01) | I/O error → exit 2 | `observe.rs` `never_follows_a_symbolic_link`; `agent.rs` `observes_the_project_on_disk` |
| AppPort discovery | `collect::discover`: the app's own `appport.manifest@1` over IPC, **no permissions** | app does not start → exit 2 | `loop.rs` `the_published_manifest_matches_the_offline_fixture`; `parity.rs`; drift check |
| plan | `agent_core::plan::plan`: target, boundary, editable files, least-privilege permissions, probes, checks; absent declarations are "unknown", never "no" | `refused to plan: …` → `limitation` | `agent.rs` (plan snapshot, refusals); `knowledge.rs` `absent_declarations_are_unknown_never_no`; `readiness.rs` `an_unverifiable_contract_is_a_limitation` |
| pre-model gates | `run.rs`: project lock (RA-06), uncommitted-work check (RA-05), baseline run, goal-evidence decision (RA-03) | `limitation` before any model call; a second run → exit 2 | `readiness.rs` `ra05_…`, `ra06_…`, `ra03_preserving_…` |
| model request | `agent_model` providers; bounded retries (RA-08); refusal or empty answer = provider failure (RA-09) | `provider-error` (exit 4), project unchanged | `reliability.rs` (7); `providers.rs`; `readiness.rs` `a_provider_failure_is_reported_as_such` |
| authorization | the plan's editable files (`plan.editable()`); app permissions come from the contract's `authorization`; the model cannot add files or permissions | outside the boundary → not written; fed back | `loop.rs` `an_edit_outside_the_boundary_is_never_written`; `agent.rs` `judges_the_edit_boundary` |
| file changes | `edit::apply`: every target checked first; no links; regular files only; byte-exact originals; atomic write; recovery record written before running (RA-10) | refused before writing → fed back; a failed write is undone and checked | `edit.rs` (8 unit tests); `readiness.rs` `ra10_…` |
| execution | `host::Host::start`: the project's app behind `AppPortConnection`, under exactly the plan's permissions, **without credentials** (RA-02) | the candidate cannot load → every runtime check fails | `loop.rs` `an_implementation_that_does_not_load_fails_every_runtime_check`; `readiness.rs` `ra02_…` |
| tests | `collect::collect`: regression probes on existing state, then the target's flows and their declared verification methods | a missing flow or method fails its check; missing ≠ passed | `agent.rs` `judges_outputs_and_regressions_…`; `loop.rs` `a_contradicted_verification_…` |
| independent verification | `agent_core::verify::judge`: every listed check must pass; the edit boundary is computed from what was written, not claimed | rejected → checked rollback → retry with violations | `agent.rs` `judges_contract_identity_from_the_published_manifest` |
| goal | `goal::run_goal_check` (fresh process; must fail before the change) or declared verification failing before and passing after | goal not met → rejected and rolled back | `readiness.rs` `ra03_…` (3 tests); `arms.rs` `an_unchanged_project_never_achieves_a_goal` |
| completion report | `RunReport`: outcome, reason, `projectState`, `goal`, per-attempt rollback, `inFlight`; redacted; written atomically | see [outcomes](agent-loop.md#outcomes-and-exit-status) | `readiness.rs` (every test reads the report and the exit code) |

**Docs compared with the code.** One documented claim was false before this PR: `agent-loop.md` said keys are "never written to … the log". Model-written application code could read them, and the log recorded what it returned (RA-02). The claim is now true and tested, and the docs say why.

## 4. Findings

**IDs** are stable. **Base** says whether the problem reproduces on `8bf8e02`. **Regression** names the test that now guards it.

### P0: critical (all fixed)

#### RA-01: writes and reads followed symbolic links out of the project

- **Scenario.**
  - A project whose handler `src/handlers/customers.lookup.mjs` is a symlink to a file elsewhere: a shared-source layout, or a hostile repository.
  - `observe` walked links to files as ordinary sources, and `edit::apply` / `rollback` wrote through them. `normalize` only checks paths lexically.
  - Reads through the link were sent to the model as context.
- **Base evidence.** Reproduced with the base binary on a copy of `examples/customers-app`:
  - `appport-agent run … --provider scripted` exited **0, `accepted`**;
  - the checksum of the file *outside* the project changed (`sha256sum -c` → `FAILED`);
  - the rejected first attempt and its "rollback" also went through the link.
- **Impact.** Writes to arbitrary files the user can write; secrets read into model context.
- **Fix.**
  - `observe` reports links (`source.symlink`) and never follows them.
  - `observe::inside` checks every path component for links.
  - `Project::read` and `edit::apply` / `rollback` refuse any path through a link.
  - `apply` writes only regular files.
- **Regression.** `observe.rs` `never_follows_a_symbolic_link`; `edit.rs` `never_writes_through_a_symbolic_link`; `readiness.rs` `ra01_a_symlink_out_of_the_project_is_never_followed`.
- **After.** The same reproduction ends with `limitation` (exit 3): the handler cannot be located. The outside file's checksum is OK, and there are 0 model calls.

#### RA-02: model-written code could read provider credentials, and the log recorded them

- **Scenario.**
  - The application (and, in `code-eval`, the test command) runs with the agent's full environment, including `AI_GATEWAY_API_KEY`.
  - A proposal can read the key and return it. The log and the retry feedback then carry it, which sends it back to the model.
- **Base evidence.** With `AI_GATEWAY_API_KEY=vck_repro_secret_value`, a scripted handler throwing ``errors.notFound(`leak ${process.env.AI_GATEWAY_API_KEY}`)`` put the key into `run.jsonl` **9 times**.
- **Impact.** Credential exfiltration to the model, logs and artifacts.
- **Fix.**
  - `Host::start` and `coding::run_command` remove every credential variable: `CREDENTIAL_ENV` plus the variables the model configuration names.
  - `Log`, the run report and provider error bodies are redacted of those values.
- **Regression.** `readiness.rs` `ra02_secrets_never_reach_the_application_or_the_log`; `reliability.rs` `a_key_echoed_in_an_error_body_is_redacted`.
- **After.** The same reproduction: the key appears 0 times in the log and 0 times in the report, and the application saw `leak undefined`.

#### RA-03: `run` accepted changes that preserved behaviour but did not achieve the goal

- **Scenario.** This is the `billing-from-store` failure, in the production path.
  - The judge verifies preservation: same answers, same contract, declared verification.
  - When nothing observable separates "goal achieved" from "unchanged", a behaviour-preserving rewrite that ignores the goal passes.
- **Base evidence.** On `fixtures/appport-projects/customers-billing`, the base binary was given a billing rewrite that still calls the legacy lookup. It exited **0, `accepted`**.
- **Impact.** False success after unverified work.
- **Fix.**
  - `run` requires goal evidence: a `--goal-check` that fails before and passes after, or a declared verification that fails before and passes after.
  - Without either, it declines (`limitation`, exit 3) **before any model call**.
  - `--accept-preserving` opts out, and the report records it.
  - The evaluation harnesses keep scoring the goal with their oracle.
- **Regression.** `readiness.rs`:
  - `ra03_preserving_behaviour_without_achieving_the_goal_is_rejected`: the change builds, and **the deterministic judge accepts it** (`verdict.accepted: true`), but the goal check fails. It is rejected, rolled back and checked; exit 1.
  - `ra03_a_verified_change_records_its_goal_evidence`.
  - `ra03_accepting_without_goal_evidence_must_be_asked_for_and_is_recorded`.
- **Also.** `arms.rs` `an_unchanged_project_never_achieves_a_goal` (from #9).

#### RA-04: rollbacks were unchecked and could destroy work

- **Scenario.**
  - `rollback` ignored every error and was always reported as done (`rolledBack: true`).
  - Originals were read with `read_to_string`. An unreadable file was recorded as "did not exist", so a rollback would *delete* it.
  - A file the user edited while the agent was running was overwritten with the pre-run original.
- **Base evidence.** Code inspection (`edit.rs` at `8bf8e02`: `let _ = match original …`, and `original = fs::read_to_string(&full).ok()`).
- **Impact.** Silent data loss, and a false "restored" claim.
- **Fix.**
  - Originals are kept as bytes; a target that cannot be read is refused before anything is written.
  - Writes are atomic (temporary file + rename).
  - `rollback` restores and re-reads each file, and leaves alone any file that no longer holds what the agent wrote.
  - `Rollback` reports restored / left alone / failed, and is `#[must_use]`.
  - The loops report `rolledBack` only when every file checks out. Otherwise the outcome is `rollback-failed` (exit 5) with `projectState: uncertain`, and the run stops.
- **Regression.** `edit.rs`:
  - `restores_bytes_exactly_even_when_they_are_not_utf8`;
  - `never_overwrites_a_file_changed_after_the_agent_wrote_it`;
  - `checks_every_target_before_writing_any`;
  - `applies_and_rolls_back`.

  Also `readiness.rs` `a_rejected_run_exits_1_with_the_project_restored`.

### P1: high

| ID | finding | base evidence | fix in this PR | regression test |
|---|---|---|---|---|
| RA-05 | A whole-file rewrite of a file with **uncommitted changes** loses them on acceptance | code inspection: no check | in a git work tree, `run` declines (exit 3) if a file it would rewrite is modified or untracked; `--allow-dirty` overrides | `readiness.rs` `ra05_uncommitted_changes_are_never_taken_over`; `closed-loop-test.sh` stage 3 (`3-services-dirty`) |
| RA-06 | **Concurrent runs** on one project could undo each other's work | code inspection: no lock | `ProjectLock` in the temporary directory, keyed by the canonical project path; a dead holder's lock is taken over | `readiness.rs` `ra06_a_second_run_on_the_same_project_is_refused`; takeover in `ra10_…` |
| RA-07 | **Outcomes conflated.** Provider failure, harness failure and a planning refusal all exited 2 and wrote no report. A refusal after attempts lost their evidence. | `main.rs`: `run` → `Err` → exit 2 | `run` writes a report for every outcome; exit 0 accepted / 1 rejected / 2 usage or harness / 3 limitation / 4 provider / 5 rollback failed | every `readiness.rs` test asserts the exit code and the report |
| RA-08 | **No retries.** One 429 or 503 ended the run; `Retry-After` was ignored | `providers.rs`: one request | bounded retries in the provider only: exponential backoff with jitter, `Retry-After` honoured up to a cap; no retry on timeouts or on 400/401/403/404; retries do not multiply across layers | `reliability.rs` (5 tests); `readiness.rs` `a_rate_limit_retry_applies_the_change_once` (two requests, one model call, one write) |
| RA-09 | OpenAI-compatible **refusals and empty completions** counted as agent attempts ("unusable response"), so a refusal could end as `rejected` | `providers.rs`: only Anthropic refusals detected | `message.refusal`, `finish_reason: content_filter` and empty content are provider errors (classified `provider-refusal` / `provider-error`) | `reliability.rs` `refusals_and_empty_answers_are_provider_failures_not_proposals` |
| RA-10 | A run that **died mid-attempt** left modified files and no record | code inspection: report written only at the end | before the changed application runs, the report says `interrupted`, with `inFlight` (paths, SHA-256 before and written, original contents) | `readiness.rs` `ra10_an_interrupted_run_leaves_an_accurate_recovery_record`: the agent is SIGKILLed mid-candidate; the record matches the disk |
| RA-11 | Model-written code runs **unsandboxed** (filesystem, network) | by design | **not fixed**: credentials are removed (RA-02); a sandbox needs a design decision | [#10](https://github.com/rkendel1/secret-agent/issues/10). **Blocks** unattended and untrusted-repository use |
| RA-12 | **No general in-place coding workflow**: `run` handles one AppPort intent; the generic loop is evaluation-only | `main.rs` commands | **not fixed** | [#11](https://github.com/rkendel1/secret-agent/issues/11). **Blocks** primary-agent use |

### P2: medium

| ID | finding | status |
|---|---|---|
| RA-13 | A test command that timed out was killed but its children were not (e.g. `npm test` → `node`) | **fixed**: own process group; the group is killed on timeout (`coding.rs`) |
| RA-14 | Memory and context are unbounded: all sources read into memory; context files sent whole with no context-window check; one non-UTF-8 source fails observation; large repositories untested | deferred, [#12](https://github.com/rkendel1/secret-agent/issues/12). An oversized request now surfaces as `provider-error`, not as an agent outcome |
| RA-15 | No restore command, no resume, no signal handling (Ctrl-C mid-attempt leaves the change, with its record) | deferred, [#13](https://github.com/rkendel1/secret-agent/issues/13). Interruption is never reported as completion (RA-10) |
| RA-16 | The native arm's `invoke` trusts a capability's declared `observation` effect. An implementation with external side effects (a real database) would be exercised. Each invocation is a fresh process, but external state is not isolated. | documented. The native arm is evaluation-only and runs on copies. Consequential capabilities are refused (`arms.rs` asserts the guard) |
| RA-17 | A paid provider may bill a retried request. Requests that time out are not retried, for this reason and because they may still complete. | documented (`agent-loop.md`). Live runs are restricted to verified free models (#9) |

### P3: low

| ID | finding | status |
|---|---|---|
| RA-18 | `ci.yml` had no explicit `permissions` | **fixed**: `contents: read` |
| RA-19 | `agent-loop.md` claimed keys never reached the log (false before RA-02); usage text had no outcome or exit-code contract | **fixed**: `agent-loop.md` *Outcomes and exit status*, *Goal evidence*, *Safety guarantees*; README; `--help` |
| RA-20 | Actions are pinned by tag, not commit SHA | deferred, [#14](https://github.com/rkendel1/secret-agent/issues/14) |
| RA-21 | Taking over a stale lock uses `kill -0` (Unix). Elsewhere a dead run's lock must be removed by hand; the error message says where it is | documented |

### Reviewed and found sound

These are areas the spec asked about where no change was needed, with the evidence:

- **The judge fails closed.**
  - Every listed check must pass.
  - A candidate that did not run fails every check but the boundary.
  - A missing flow or verification record fails its check, and a regression probe missing after the change fails.
  - "No regression probe ran" fails.
  - Evidence: `agent-core/src/verify.rs` (`judge`, `run_check`); `agent.rs` judge tests.
- **The contract stays authoritative.**
  - Identity is the published `contractDigest` plus the target's contract pin.
  - Absent declarations are reported as unknowns.
  - A contract without effects or verification is refused (`contract.unverifiable`).
  - No second contract system exists.
  - Evidence: `knowledge.rs`, `parity.rs`, `surface.rs`, the drift check.
- **Least privilege.**
  - Discovery runs with no permissions.
  - Probes run with exactly the permissions the plan derives from the contract.
  - A goal check runs with only its own capabilities' declared permissions.
  - The model never names permissions.
- **Model output is never proof.**
  - The model writes files and nothing else.
  - Acceptance comes from running the application and the judge.
  - The native arm's `complete` is refused until a proposal is verified (`arms.rs` asserts `refusedCompletions`).
  - Every evaluation is rescored by an independent oracle.
- **Multi-turn replay.** Assistant turns are replayed byte for byte when a provider requires it (`providers.rs` `conversations_replay_assistant_turns_unchanged`).
- **Live inference never runs implicitly.**
  - `agent-live` is `workflow_dispatch` only, behind the free-model preflight, with the key scoped to the steps that call the gateway.
  - `ci.yml` has no secrets.
  - Evidence: `.github/workflows/`; `free_gateway.rs`; `gateway.rs`.

## 5. The test matrix

All deterministic, with scripted completions or a local stand-in provider; no paid inference.

| # | case | test |
|---|---|---|
| 1 | successful verified change | `readiness.rs` `ra03_a_verified_change_records_its_goal_evidence`; `loop.rs` `demo_completions_reject_then_accept` |
| 2 | compiles and preserves behaviour, but misses the goal | `readiness.rs` `ra03_preserving_behaviour_without_achieving_the_goal_is_rejected` |
| 3 | false success rejected | same test (the judge accepts; the goal check rejects); no goal evidence → declined before any model call; `arms.rs` (oracle `preserved-only`, refused `complete`) |
| 4 | missing or unverifiable capability | `readiness.rs` `an_unverifiable_contract_is_a_limitation`; `arms.rs` (`missing-capability`, `unverifiable-contract`) |
| 5 | contract identity or schema mismatch | `agent.rs` `judges_contract_identity_from_the_published_manifest`, `judges_outputs_and_regressions_against_the_contract_and_the_baseline`; `knowledge.rs` validation tests |
| 6 | unsupported consequential operation blocked | `arms.rs` (native `invoke` of a consequential capability is refused) |
| 7 | unauthorized filesystem access | `readiness.rs` `ra01_…`; `edit.rs` `never_writes_through_a_symbolic_link`, `rejects_paths_outside_the_project`; `observe.rs` `never_follows_a_symbolic_link`; `loop.rs` `an_edit_outside_the_boundary_is_never_written` |
| 8 | provider 403, 429, timeout, malformed, refusal | `reliability.rs`; `gateway.rs`; `providers.rs`; `readiness.rs` `a_provider_failure_is_reported_as_such` (403 → exit 4) |
| 9 | rate-limit retry without a duplicate effect | `readiness.rs` `a_rate_limit_retry_applies_the_change_once` |
| 10 | conversation replay | `providers.rs` `conversations_replay_assistant_turns_unchanged` |
| 11 | partial mutation, then failed verification | `edit.rs` `checks_every_target_before_writing_any`, `applies_and_rolls_back`; `readiness.rs` `a_rejected_run_exits_1_with_the_project_restored`; `loop.rs` `a_contradicted_verification_is_rejected_and_rolled_back` |
| 12 | interrupted run, accurate recovery report | `readiness.rs` `ra10_an_interrupted_run_leaves_an_accurate_recovery_record` |
| 13 | secret echo redacted | `readiness.rs` `ra02_…`; `reliability.rs` `a_key_echoed_in_an_error_body_is_redacted`; `gateway.rs` `a_key_echoed_by_the_provider_is_redacted`; `free_gateway.rs` (no record holds the key) |
| 14 | existing user changes preserved | `readiness.rs` `ra05_…`; `edit.rs` `never_overwrites_a_file_changed_after_the_agent_wrote_it` |
| 15 | accurate status and exit code | every `readiness.rs` CLI test (exit 0, 1, 3, 4, and a killed run); `closed-loop-test.sh` (exit 3 twice, then accept) |

## 6. Commands and exact results

Run on this branch on 2026-10-16 (Linux, Node.js 22, stable Rust):

| command | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test --all -- --include-ignored` | **138 passed, 0 failed** (base: 114 passed, 0 failed); every test binary reported `ok` |
| `scripts/closed-loop-test.sh` | exit 0: stages 1 and 2 refused with no model call; stage 3 declined twice (uncommitted changes; no goal evidence), then accepted with `--allow-dirty --accept-preserving` after 1 rejection and 1 rollback; stage 4 accepted after 1 rejection and 1 rollback |
| `scripts/check-coding-fixtures.sh` | exit 0 |
| `appport-agent code-eval --tasks fixtures/coding --provider scripted --trials 1 --arms observed --min-success 1 --require-test-runs` | exit 0 |
| `scripts/check-appport-drift.sh --pinned` | exit 0 |
| `appport-agent preflight --config examples/models/vercel-gateway.json` (no key in this sandbox) | exit 1: `FAIL (config) … AI_GATEWAY_API_KEY is not set`; no request sent |

Test counts per binary:
- agent-core: 17 unit, 7, 9, 4, 17, 10, 4;
- agent-model: 2 unit, 9 gateway, 10 providers, 7 reliability;
- appport-agent: 16 unit, 2 arms, 1 coding, 4 free_gateway, 7 loop, 12 readiness.

**Pre-existing failures:** none. The base commit was fully green. Two existing expectations changed, intentionally:
- **`closed-loop-test.sh` stage 3.** It now asserts that `run` declines by default. The `appport-services` target file is untracked after `appport init`, and nothing observable can show that goal. The old acceptance path is still exercised, with explicit flags.
- **`providers.rs` `provider_errors_are_reported_not_swallowed`.** It now sets `maxRetries: 0`, because a 429 is retried by default.

## 7. Model and provider details for live tests

**No live inference was run for this PR:**
- this sandbox has no `AI_GATEWAY_API_KEY`;
- the session's GitHub App cannot dispatch workflows (HTTP 403).

The path for it is unchanged from #9: the **agent-live** workflow, `mode: smoke`, model `nvidia/nemotron-3.5-lightning-free` through `https://ai-gateway.vercel.sh/v1`. The preflight checks the allowlist, zero catalog pricing, and one minimal completion before any trial. There is no Anthropic or paid fallback.

The deterministic results above do not depend on a provider. The live run would show how a small free model performs, which no test here claims.

## 8. Operational limits and safeguards

- **Generated code runs unsandboxed** when the application or a test starts. It runs without provider credentials, but with your filesystem and network access (RA-11).
- **`run` changes the project in place.** Use git:
  - a rejected run leaves the project checked as unchanged;
  - an accepted run leaves the change for you to review with `git diff`;
  - `rollback-failed` (exit 5) or a report still saying `interrupted` means: read `reason` / `inFlight` and restore by hand. There is no restore command yet (#13).
- **Goal evidence.** Write a `--goal-check` for any goal the contract's declared verification does not already observe. `--accept-preserving` is for refactors whose goal *is* "behave the same".
- **One run per project at a time.**
- **Provider retries.** At most `maxRetries` (default 2) per model call, each wait capped at `maxRetryWaitSecs` (default 30). Timeouts are never retried; set `timeoutSecs` to bound a call.
- **Reproducibility.** Every run can write a report (`--report`) and a step log (`--log`), both redacted. The report is written atomically.

## 9. Blocked and permitted uses

**Permitted today (supervised):**
- `appport-agent plan`: deterministic planning of a capability replacement, with no model and no change.
- `appport-agent run` on a JavaScript/TypeScript AppPort project in a git work tree, for `replace <capability>@<version>`:
  - with a `--goal-check`, or where a declared verification fails before the change;
  - on a machine where running model-written code is acceptable;
  - with the result reviewed before it is committed.
- Evaluations (`eval`, `arm-eval`, `code-eval`) on copies: scripted in CI, live only through the free-gateway preflight.

**Blocked:**
- Primary or general-purpose coding: anything other than one capability replacement (#11).
- Unattended runs, and any untrusted repository or model output: no sandbox (#10).
- Projects whose sources are reached through symbolic links. They are refused; this is by design.
- Large monorepos, until #12.
- Accepting a change as "done" with no evidence of the goal, unless `--accept-preserving` is passed deliberately.
- Claims about smaller or free models' effectiveness, until the live smoke and comparison runs have results.
