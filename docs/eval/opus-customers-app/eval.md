# Closed-loop evaluation

- intent: `replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export`
- project: `examples/customers-app`
- model: `anthropic:claude-opus-5-5 via https://api.anthropic.com/v1/messages`
- attempts per trial: up to 3
- run: 2026-10-15T23:24:54.760Z → 2026-10-15T23:30:21.535Z

## Outcomes

| arm | trials | accepted on 1st attempt | accepted after retry | rejected (all attempts) | errors | acceptance rate | mean attempts to accept |
|---|---|---|---|---|---|---|---|
| knowledge | 10 | 10 (100%) | 0 | 0 | 0 | 100% | 1.00 |
| bare | 10 | 0 (0%) | 0 | 9 | 1 | 0% | – |

## Cost and behaviour

| arm | model calls | tokens in / out | model calls per accepted | tokens per accepted | rejected attempts | rollbacks | contract edits | boundary violations | unusable responses | mean wall time |
|---|---|---|---|---|---|---|---|---|---|---|
| knowledge | 10 | 26930 / 4176 | 1.00 | 3111 | 0 | 0 | 0 | 0 | 0 | 5.3 s |
| bare | 27 | 23128 / 20605 | – | – | 27 | 19 | 0 | 0 | 8 | 23.9 s |

## Why proposals were rejected

| arm | check | rejections |
|---|---|---|
| bare | `behavior.regression` | 18 |
| bare | `contract.identity` | 18 |
| bare | `contract.valid` | 18 |
| bare | `failures.declared` | 18 |
| bare | `output.conformance` | 18 |
| bare | `response.format` | 8 |
| bare | `runtime.load` | 18 |
| bare | `verification` | 19 |

## Trials

| arm | trial | outcome | attempts | rejected by | tokens in / out | wall time |
|---|---|---|---|---|---|---|
| knowledge | 1 | accepted | 1 | – | 2693 / 507 | 6.0 s |
| knowledge | 2 | accepted | 1 | – | 2693 / 432 | 5.4 s |
| knowledge | 3 | accepted | 1 | – | 2693 / 395 | 4.9 s |
| knowledge | 4 | accepted | 1 | – | 2693 / 381 | 5.2 s |
| knowledge | 5 | accepted | 1 | – | 2693 / 406 | 5.3 s |
| knowledge | 6 | accepted | 1 | – | 2693 / 430 | 5.3 s |
| knowledge | 7 | accepted | 1 | – | 2693 / 411 | 5.1 s |
| knowledge | 8 | accepted | 1 | – | 2693 / 435 | 6.1 s |
| knowledge | 9 | accepted | 1 | – | 2693 / 410 | 5.2 s |
| knowledge | 10 | accepted | 1 | – | 2693 / 369 | 4.9 s |
| bare | 1 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2017 | 23.9 s |
| bare | 2 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2056 | 24.4 s |
| bare | 3 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 1985 | 23.0 s |
| bare | 4 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2028 | 23.5 s |
| bare | 5 | error: unexpected model response: the model declined (stop_reason refusal, category "cyber") | 0 | – | 0 / 0 | 0.0 s |
| bare | 6 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2053 | 26.1 s |
| bare | 7 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2172 | 24.2 s |
| bare | 8 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 2419 / 2019 | 23.9 s |
| bare | 9 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification | 2432 / 3411 | 36.4 s |
| bare | 10 | rejected | 3 | runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, runtime.load, contract.identity, contract.valid, output.conformance, failures.declared, behavior.regression, verification, response.format | 3763 / 2864 | 33.8 s |
