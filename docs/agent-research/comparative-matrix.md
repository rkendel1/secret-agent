# Comparative matrix

Eight open-source coding agents, read from source at pinned commits (method: [`README.md`](README.md)). Each cell gives:
- what the project does for the primitive, in a phrase;
- the decision for secret-agent: **A**dopt, **Ad**apt or **R**eject.

The full reasoning, file paths and line ranges are in [`sources/`](sources/). Every row below links to its review.

## Projects

| project | repository | commit inspected | license | review |
|---|---|---|---|---|
| Aider | https://github.com/Aider-AI/aider | `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` | Apache-2.0 | [aider-swe-agent.md §1](sources/aider-swe-agent.md#1-aider) |
| SWE-agent | https://github.com/SWE-agent/SWE-agent | `3ea751c087f32b16e039a2233dd6eefecef325d5` | MIT | [aider-swe-agent.md §2](sources/aider-swe-agent.md#2-swe-agent) |
| OpenHands | https://github.com/All-Hands-AI/OpenHands (now the frontend only); agent: https://github.com/OpenHands/software-agent-sdk; benchmarks: https://github.com/OpenHands/benchmarks | `aa593c72103862d4f1851988ecacd30f514368dc`; SDK `db9c54b71c8c9ed46a2454fc65eee804d3c1e8aa`; benchmarks `405bae7140d7e961a75f4910a0b2e7069731db96` | MIT | [openhands-opencode.md §1](sources/openhands-opencode.md#1-openhands) |
| OpenCode | https://github.com/sst/opencode | `e25b7abf8955a4fa1b02b935964cb47db4c3a02a` (the Effect-based rewrite) | MIT | [openhands-opencode.md §2](sources/openhands-opencode.md#2-opencode) |
| Cline | https://github.com/cline/cline | `23791b9323aadd91d7dbaea8a8fa262cd2f7ad0b` (the `sdk/` rewrite) | Apache-2.0 | [cline-goose.md §1](sources/cline-goose.md#1-cline) |
| Goose | https://github.com/block/goose | `21d873eec92ad2f0cd7f39d00680b4e3dbc41f9a` | Apache-2.0 | [cline-goose.md §2](sources/cline-goose.md#2-goose) |
| Pi | https://github.com/badlogic/pi-mono (`packages/coding-agent`, `packages/agent`, `packages/evals`) | `4954d760504c11df9b2cbdc040346107b6a8153d` | MIT | [pi-gemini.md §1](sources/pi-gemini.md#1-pi-pi-mono-packagescoding-agent--packagesagent) |
| Gemini CLI | https://github.com/google-gemini/gemini-cli | `21976e977ef833e9a03538119eac933fe89767a7` | Apache-2.0 | [pi-gemini.md §2](sources/pi-gemini.md#2-gemini-cli) |

No code was copied from any project. Every adoption below is a re-implementation of an idea, so no license obligation arises. If code is ever ported, check its license and attribute it.

## Matrix

| primitive | Aider | SWE-agent | OpenHands | OpenCode | Cline | Goose | Pi | Gemini CLI |
|---|---|---|---|---|---|---|---|---|
| **Repository intelligence** | tree-sitter def/ref graph, personalized PageRank, token-fitted repo map — **Ad** | grep/find tools; Python body-eliding filemap — **R** | ripgrep listing by mtime, grep — **R** | glob/grep tools; nearest AGENTS.md on touched paths — **Ad** | ripgrep file index (tree-sitter removed) — **Ad** | tree-sitter analyzer + name-linked call graph, in Rust — **A** | rg/fd tools; ancestor AGENTS.md — **Ad** | glob by recency; GEMINI.md just in time per directory — **Ad** |
| **Context selection and budget** | full boundary files + map fitted by binary search; LLM history summary — **Ad** | deterministic history processors — **A** | LLM condenser; head+tail truncation, full log kept — **Ad** | token ceiling + 0.7/0.5/0.35 shrink ladder — **Ad** | deterministic compaction, head/tail truncation — **Ad** | compaction behind small traits — **A** (the trait split) | compaction; chars/4 estimator — **R** / **Ad** | compression at 50%, discarded if it grows — **Ad** |
| **Planning** | architect → editor free-text plan — **R** | prompt-only workflow — **R** | model task tracker; plan agent limited to its plan file — **R** | plan mode = a permission ruleset — **Ad** (the mechanism) | plan/act modes — **R** | todo nudges; recipes — **R** / **Ad** (recipe as arm spec) | none (by design) — **R** | plan mode enforced by policy — **R** / **A** (the mechanism) |
| **Editing format** | many formats; whole-file filename recovery; failed-match messages — **Ad** | `str_replace` (exactly one match); windowed edit reverts on new lint errors — **Ad** | `str_replace` editor, path checks — **R** (format) | edit/patch/write by model; verify all hunks before writing — **Ad** | diff blocks; absolute paths accepted — **R** (format), **A** (fail closed) | whole-file `write` — **A** | `edit` (unique, non-overlapping), `write`; refuses truncated output — **A** (truncation rule) | replace + fuzzy tiers; placeholder detector — **A** (detector), **R** (fuzzy) |
| **Recovery** | reflect up to 3×, same approach — **Ad** | typed exits; ephemeral requery; hard reset per outer attempt; LLM chooser — **Ad** / **R** (chooser) | stuck detector: one nudge, then stop — **A** | transport retry apart from attempts; step cap — **A** | loop detector warn 3 / stop 5; mistake limit — **A** | check-gated recipe retry, reset to first message — **A** (as fresh-retry arm) | provider backoff; failed attempt hidden — **Ad** | loop detection: one correction, then stop — **A** |
| **Verification** | auto-lint, auto-test, feed output back — **A** | model `submit` + LLM review — **R** | `finish` + critics — **R** | none; model decides when done — **R** | model's own `verified` flag — **R** | exit-code checks in recipes — **A** | none in core — **R** | prompt only — **R** |
| **Execution policy** | shell only with explicit yes; no sandbox — **Ad** | Docker per instance; prefix denylist — **Ad** | resolved-path allow-list; container — **Ad** | (action, pattern, allow/deny/ask), deny wins — **A** | auto-approve by default — **Ad** | `ToolInspector` pipeline; auto by default — **A** / **R** | no approvals; trust gates files only — **Ad** | layered policy engine; path checker; build-file taint; sandboxes — **A** |
| **Tool interface** | edit-format parsers per coder — **A** (pattern) | declarative tools, strict one-call parse, typed errors — **Ad** | typed tool schemas — **Ad** | schema-typed outputs — **Ad** | per-tool metadata (retryable, timeout) — **Ad** | `Operation`/`StepResult` pipeline; minimal `Provider` — **A** | read/bash/edit/write — **R** (model surface) | declarative tools; untrusted wrapping of outputs — **A** (wrapping) |
| **Evaluation harness** | `pass_rate_k`; pristine tests re-copied; one shared container — **A** | fresh container per instance, external scoring, replay — **A** | inference vs scoring separated; diff vs base — **Ad** | record/replay HTTP cassettes — **Ad** | pass@k, pass^k, flakiness; substring checks — **A** / **R** | Harbor: config per arm, container per trial — **A** | paired arms, blocked pairs, flaky/saturated flags — **A** | pass-rate labels; skips on API error — **Ad** / **R** |
| **Own tests** | parsers strong; retry loop untested — **Ad** | exit statuses strong; retry/reviewer untested — **Ad** | scripted model drives the loop — **A** | permission-precedence tests — **A** | scenario runtime tests — **Ad** | scripted-provider pipeline harness — **A** | loop-termination tests — **A** | table-driven loop/policy tests — **A** |

## What the reading established

1. **No reference agent decides success by running the project's tests on its own.** Six of the eight let the model declare completion (SWE-agent `submit`, OpenHands `finish`, Cline `verified`, OpenCode, Pi, Gemini CLI). Aider runs tests but leaves the verdict to the user. Goose checks exit codes only in optional recipe retries. Secret-agent's rule (acceptance only from the project's tests, plus hidden acceptance in evaluation) has no counterpart to copy, and it is kept.
2. **The best recovery idea appears three times, independently.** OpenHands, Gemini CLI and Cline each detect a repeated signature and stop, after one warning in the first two. This is now `stop-on-repeat`. The one-warning step is the next variant to test.
3. **Repository intelligence worth having is structural.** That means tree-sitter definitions and references, ranked (Aider) or graphed (Goose, in Rust). Secret-agent's `dependency-aware` strategy is a lexical first step, and labels itself so in its unknowns. The tree-sitter version is the first follow-up experiment.
4. **The strongest evaluation designs** pair the arms, isolate each trial, alternate the order, and account for runs that failed for infrastructure reasons instead of dropping them (Pi, SWE-agent, Goose Harbor). The experiment harness follows them, except for containers: trials get fresh copies, not containers (see [`decisions.md`](decisions.md)).
