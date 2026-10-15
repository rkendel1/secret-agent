# appport-services, as an agent target

`setup.sh <dir> [clone|init|declared]` prepares
[`rkendel1/appport-services`](https://github.com/rkendel1/appport-services) at
commit `81426d6` for `scripts/closed-loop-test.sh`:

- `clone`: as published. It has no AppPort application runtime; the agent refuses.
- `init`: `npx appport@1.1.20 init`, plus `@appport/sdk@1.2.4` and `@appport/schema@1.1.0`.
  The generated contract declares no effects or verification; the agent refuses
  to accept changes it cannot verify.
- `declared` (default): plus `declare-contract.patch`, the declarations a
  contract author adds so changes can be verified.

`scripted/replace-documents-open/` holds pre-written completions for
`--provider scripted`. They are not model output. `01.txt` returns a copy but also
adds `openedAt` to the output schema; the runtime serves it, and the agent rejects
it on `contract.identity` and `behavior.regression`. `02.txt` is the change without
the contract edit.
