# Agent research: method and architecture

This directory is a maintained engineering artifact. It records:
- what eight open-source coding agents actually do, read from their source at pinned commits;
- what secret-agent takes from them, and how each idea is measured before it is kept.

| file | what |
|---|---|
| [`primitives.md`](primitives.md) | the primitive inventory: what exists in secret-agent, where, and who owns each responsibility |
| [`comparative-matrix.md`](comparative-matrix.md) | OpenCode, Aider, SWE-agent, OpenHands, Cline, Goose, Pi and Gemini CLI, primitive by primitive, each with adopt / adapt / reject |
| [`decisions.md`](decisions.md) | architecture decisions, the alternatives considered, and why |
| [`experiments.md`](experiments.md) | baselines, hypotheses, the benchmark method, and how to add a strategy or an experiment |
| [`sources/`](sources/) | the per-project reviews, with file and line citations for every claim |

## Method

1. **Read the source, not the description.** Each project was cloned with `git clone --depth 1` at the commit listed in the matrix and read only. Nothing from it was installed or run. A claim about a project is admissible only with a path (and usually a line range) at that commit.
2. **The same ten primitives for every project:**
   1. repository intelligence;
   2. context selection and budget;
   3. planning;
   4. editing format;
   5. recovery;
   6. verification;
   7. execution policy;
   8. tool interface;
   9. evaluation harness;
   10. the project's own tests.
3. **One decision per primitive:** adopt, adapt or reject, with one reason, judged against secret-agent's invariants:
   - the deterministic core plans and draws the edit boundary;
   - the project's own tests (or AppPort contract checks) decide acceptance;
   - model output is never proof;
   - permissions and the boundary never widen on retry.
4. **Spot checks.** The reviews were drafted two projects at a time by research agents and then spot-checked against the clones before inclusion. Checked:
   - every commit SHA;
   - Aider's unreachable fuzzy match (`aider/coders/editblock_coder.py` L183, an early `return`) and `max_reflections = 3` (`base_coder.py` L101);
   - SWE-agent's per-attempt `hard_reset` (`sweagent/agent/agents.py` L325);
   - Gemini CLI's loop-detection service (`packages/core/src/services/loopDetectionService.ts`);
   - Cline's SDK layout (`sdk/packages/...`).

   The remaining line ranges are as recorded by the reviewers. Re-verify a citation before relying on it for a design change.
5. **Ideas become experiments, not features.** An idea worth taking becomes a strategy behind an existing boundary, and is kept only if the experiment harness shows it improves verified outcomes (see [`experiments.md`](experiments.md)).

**Updating:** re-clone at a newer commit, update the commit in the matrix and the review, and re-check every citation the decisions rely on.

## Architecture before this work (the audit)

What existed, read from the code at `main` `192185d`:

| responsibility | where | coupled to |
|---|---|---|
| repository observation | `agent_core::observe`: a static JS/TS scan of sources, imports, AppPort declarations; symlinks are never followed | — |
| AppPort knowledge | `agent_core::appport::knowledge`: manifest, graph, `CapabilityView`, `ReplacementBoundary`, validation | — |
| planning (AppPort) | `agent_core::plan`: target, boundary, permissions, probes, checks; refuses when ungrounded | the prompt (`task_for`) |
| planning (ordinary repos) | `agent_core::generic::plan_coding`: failing tests → their imports = editable files; name matches are read-only context | **context selection** (the planner also picked read-only context, under a 48 KB budget) |
| context selection | `generic::coding_task` with a two-valued `CodingContext` (`observed`, `minimal`); `plan::task_for` with `ContextMode` (`knowledge`, `bare`) | hard-wired to the prompt layout |
| editing | `appport_agent::edit`: whole-file blocks, boundary check, checked atomic writes, checked rollback | — |
| recovery | inline in `run_coding` / `run`: retry with verifier feedback until attempts run out | hard-wired in the loops |
| verification | `agent_core::verify::judge` (AppPort); the project's test command (`coding::run_command`); hidden tests in evaluation | — (authoritative) |
| execution policy | the plan's editable files; least-privilege AppPort permissions; credentials removed from child processes; project lock; uncommitted-work refusal | — (authoritative) |
| evaluation | `code-eval` (`coding::coding_eval`: arms = `CodingContext` values), `arm-eval` (three-arm AppPort), `eval` | the arms were context modes only |

**Limitations that blocked experiments:**
- **Context and recovery could not vary.** The context had two hard-coded values, and recovery was fixed.
- **Little record of choices.** Nothing recorded *what* context a model was shown, or why.
- **Coarse outcomes.** `code-eval` reported only `accepted` / `rejected` / `refused` / `error`.
  - A model-provider failure and a missing repository both ended as `error`.
  - A run killed by a provider error lost its attempt record.
- **No experiment description.** There was no versioned description of an experiment, no baseline-vs-treatment comparison, and no ablation.
- **Partial coverage of run controls.** Limits, balanced order, provenance and the free-gateway preflight existed in `arm-eval` only.

## Architecture after this work

```
                      authorities (unchanged)                         experimental variables
 ┌──────────────────────────────────────────────────────┐     ┌──────────────────────────────────┐
 │ observe ─► plan_coding (editable files, test command)│     │ ContextStrategy                  │
 │                                                      │────►│  minimal │ observed │ dependency-│
 │ edit::apply (boundary, links, atomic, checked)       │     │  aware │ appport-aware           │
 │ run_command (the project's tests, no credentials)    │     │ → ContextSelection (items,       │
 │ hidden acceptance tests (evaluation only)            │     │   reasons, bytes, unknowns)      │
 │ edit::rollback (checked)                             │◄────│ RecoveryPolicy                   │
 └──────────────────────────────────────────────────────┘     │  feedback │ expand-context │     │
                 ▲                                            │  stop-on-repeat                  │
                 │ verdicts (tests passed? hidden passed?)    │ → RecoveryDecision (recorded)    │
                 │                                            └──────────────────────────────────┘
 coding_eval (the existing harness) ◄── experiment spec (secret-agent.experiment/1)
   fresh copy per trial · balanced order · limits · free-gateway preflight · provenance
   · fixture digests · categories · baseline comparisons · ablations
```

- **What a strategy can do.** It chooses what the model sees, and what to do after a rejection: retry, add read-only context, or stop.
- **What it cannot do:**
  - change the editable files;
  - grant a permission;
  - change the acceptance command;
  - accept an attempt.
- **Enforcement:**
  - by construction: `RecoveryDecision` has no variant that could express any of these;
  - by the loop: files a recovery policy asks for are filtered against the editable set, which comes only from the planner;
  - by tests: `tests/experiments.rs`, `recovery_policies_are_recorded_and_never_widen_the_boundary`.

Code: `crates/appport-agent/src/strategy.rs` (the strategies), `coding.rs` (the loop and the harness), `experiment.rs` (the specification).
