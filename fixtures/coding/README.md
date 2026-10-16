# Coding fixtures (no AppPort)

Ordinary repositories for `appport-agent code-eval`. Nothing here uses AppPort.

Each task directory holds:

| path | what | seen by the agent |
|---|---|---|
| `task.json` | `intent` (what a person would ask), `test` (the command that must pass), `hidden` (the acceptance command), `intendedFiles` (what a person would change) | `intent` and `test` only |
| `repo/` | the starting state: source, tests, decoys | yes |
| `hidden/` | acceptance tests run after the agent finishes, to catch changes that only satisfy the visible tests | no |
| `golden/` | a reference solution, used only by `scripts/check-coding-fixtures.sh` | no |
| `scripted/` | pre-written completions for `--provider scripted` (deterministic CI); not model output | — |

`scripts/check-coding-fixtures.sh` checks every fixture is sound: the visible
tests fail at the start, and the golden solution passes the visible and the
hidden tests.

| task | kind | what it takes |
|---|---|---|
| `bugfix-invoice-discount` | bug fix | a percentage discount is applied as a fraction; the failing test points at it among decoy modules |
| `feature-slug-max-length` | feature | add `slugify(text, { maxLength })`: word-boundary truncation, no trailing hyphen, a word longer than the limit is cut |
| `appport-readback` | bug fix | the customers application (an AppPort application) as an ordinary repository: a newly registered customer cannot be looked up, because the lookup reads a stale legacy export; hidden tests check existing customers, `NOT_FOUND`, and billing for new customers. Its `node_modules` links to `examples/customers-app` (run `npm ci` there). It is where the `appport-aware` context strategy can matter. |
