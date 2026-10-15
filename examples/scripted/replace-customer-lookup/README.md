# Scripted completions: replace `customers.lookup@1`

Pre-written model completions for `--provider scripted`, replayed in order, so the
loop in `scripts/demo.sh` is reproducible without a model or an API key. They are
not model output; they stand in for it.

- `01.txt` reads from the store but signals a missing customer with a plain
  `Error`. The runtime turns that into `INTERNAL_ERROR`, which the contract does
  not declare (`failures: NOT_FOUND`) and which changes the answer existing
  callers get. Nothing in the runtime rejects it; the agent's checks do.
- `02.txt` is the same change with the declared failure.

Point the same command at a real model with `--provider openai-compatible` (a
local LLM) or `--provider anthropic`; nothing else changes.
