# Primitive inventory

For each primitive:
- what secret-agent has, and where;
- who owns it: an **authority** (fixed; model output cannot change it) or a **strategy** (an experimental variable behind an interface);
- what the reference agents suggest trying next (see [`comparative-matrix.md`](comparative-matrix.md)).

| # | primitive | in secret-agent today | owner | experimental status | next candidates (source) |
|---|---|---|---|---|---|
| 1 | Repository intelligence | `agent_core::observe`: sources, the import graph, AppPort declarations and handlers, findings for what it cannot resolve; symlinks reported, never followed | authority (facts) | consumed by context strategies | a tree-sitter definition/reference graph with personalized PageRank (Aider `repomap.py`); tree-sitter call graph in Rust (Goose `analyze/`) |
| 2 | Context selection and budget | `strategy::ContextStrategy`: `minimal`, `observed` (baseline), `dependency-aware`, `appport-aware`. Each returns a `ContextSelection`: every item with its reason, bytes and whether it fit the budget, plus unknowns. | **strategy** | 4 implementations, selectable per arm | a budget-fitted repo map (Aider); a head-and-tail test-output digest (OpenHands `truncate.py`, OpenCode compaction ladder); nearest instruction files on touched paths (OpenCode, OpenHands) |
| 3 | Planning | `agent_core::generic::plan_coding` (ordinary repos), `agent_core::plan::plan` (AppPort): deterministic, refuses rather than guesses | authority | not a strategy: the plan defines the boundary every arm shares | none adopted: model-written plans (Aider architect, SWE-agent prompt workflow) are rejected as unverifiable |
| 4 | Editing | `edit`: whole-file blocks, a single-fence fallback, boundary and link checks, atomic writes, checked rollback | authority (the write) / single format (the parse) | **extension point, one implementation.** A second format would be a parse step in front of `edit::apply`, which stays the only writer. | search/replace with exactly-one-match and overlap checks (Pi `edit`, Gemini `replace`, SWE-agent `str_replace`); pre-check placeholder text and truncated output before tests (Gemini, Pi) |
| 5 | Recovery | `strategy::RecoveryPolicy`: `feedback` (baseline), `expand-context`, `stop-on-repeat`. Every decision is recorded per attempt. | **strategy** | 3 implementations, selectable per arm | one warning, then stop, on a repeated failure signature (OpenHands stuck detector, Gemini loop detection, Cline loop detection); reset to a fresh copy per retry (SWE-agent `RetryAgent`, Goose recipe retry) |
| 6 | Verification | the project's own test command, run by the harness, exit code captured; hidden acceptance tests in evaluation; for AppPort, `verify::judge` and goal checks | **authority** | categories exposed to the harness (`verified-success`, `accepted-unverified`, …); never selectable | diff diagnostics before and after an edit, and feed back only *new* errors (SWE-agent `windowed_edit_linting`) |
| 7 | Execution policy | the editable files from the planner; least-privilege AppPort permissions; no provider credentials in child processes; project lock; refusal of uncommitted work; the free-gateway preflight | **authority** | not a strategy | an allowlist fixed at plan time (rejects SWE-agent's prefix denylist); escalate re-running build commands after build files change (Gemini policy); OS sandbox (Gemini, SWE-agent; follow-up rkendel1/secret-agent#10) |
| 8 | Tool interface | none for the coding loop (the model only writes files). The AppPort-native arm has a 7-tool interface (`native.rs`). | — | not in scope | strict one-call validation with typed parse errors (SWE-agent `parsing.py`), if a tool-using arm is added |
| 9 | Evaluation harness | `coding::coding_eval`, driven by `code-eval` flags or an experiment spec (`experiment.rs`): a fresh copy per trial, balanced order, limits, preflight, provenance, fixture digests, categories, baseline comparison, ablation | authority (scoring) | the arms are the variables | paired arms with blocked-pair accounting and flaky/saturated flags (Pi `packages/evals`); pass@k / pass^k (Cline evals); `pass_rate_k` (Aider benchmark); a fresh container per trial (SWE-agent, OpenHands benchmarks) |
| 10 | Tests | `tests/experiments.rs` (strategies and harness), `readiness.rs` (safety), `coding.rs`, `loop.rs`, `arms.rs`, `free_gateway.rs`; unit tests in `strategy.rs`, `experiment.rs`, `edit.rs` | — | — | — |

**Coupling that was removed:** `generic::coding_task` laid out the prompt and chose its content in one function. It now builds a `TaskView` and calls `render_coding_task`. A strategy produces the view, and the baseline strategies render byte-identical prompts (`tests/experiments.rs`, `the_baseline_strategies_reproduce_the_existing_prompts_byte_for_byte`).

**Deliberately not interfaces:**
- planning and verification are authorities;
- editing has a single implementation;
- execution policy is an authority.

A trait with one implementation and no experiment behind it would only add indirection.
