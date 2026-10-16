# Three-arm evaluation: bare vs context-assisted vs AppPort-native

- model: `claude-opus-5-5`
- proposals per trial: up to 3 (every arm); native model calls per trial: up to 12
- budget: $10.00
- run: 2026-10-16T00:25:22.611Z → 2026-10-16T00:46:03.843Z

## Effectiveness

| arm | trials | verified successes (positive tasks) | first proposal | after retry | safe outcomes (negative tasks) | correct limitations | false successes | agent failures | provider / harness failures |
|---|---|---|---|---|---|---|---|---|---|
| A-bare | 30 | 3/15 (20%) | 1 | 2 | 15/15 (100%) | 10 | 0 | 12 | 0 |
| B-context | 30 | 10/15 (67%) | 10 | 0 | 10/15 (67%) | 10 | 5 | 10 | 0 |
| C-native | 30 | 10/15 (67%) | 10 | 0 | 15/15 (100%) | 15 | 0 | 5 | 0 |

## Efficiency

| arm | model calls | calls per verified success | tokens in / out | tool calls | AppPort bytes to model | application runs | rejected proposals | rollbacks | cost | wall s (q1 / median / q3) | tokens per trial (median) | calls per trial (median) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A-bare | 42 | 14.00 | 27780 / 41473 | 0 | 0 | 64 | 31 | 31 | $0.94 | 0.0 / 7.8 / 32.1 | 813 | 0 |
| B-context | 15 | 1.50 | 35025 / 13247 | 0 | 18125 | 45 | 0 | 0 | $0.41 | 0.0 / 2.7 / 10.8 | 1340 | 0 |
| C-native | 141 | 14.10 | 410285 / 27966 | 140 | 171101 | 69 | 0 | 0 | $2.20 | 12.5 / 17.1 / 24.7 | 8320 | 4 |

## Differences

| comparison | verified rate | safe rate | median wall s | median tokens per trial |
|---|---|---|---|---|
| C-native − B-context | +0 pp | +33 pp | +14.4 (+539%) | +6979.0 (+521%) |
| B-context − A-bare | +47 pp | -33 pp | -5.2 (-66%) | +527.5 (+65%) |
| C-native − A-bare | +47 pp | +0 pp | +9.2 (+118%) | +7506.5 (+923%) |

## By task

| task | arm | trials | outcomes | model calls | tokens in / out | median wall s |
|---|---|---|---|---|---|---|
| `billing-from-store` | A-bare | 5 | unverified 2, verified-success 3 | 12 | 6421 / 12415 | 33.0 |
| `billing-from-store` | B-context | 5 | verified-success 5 | 5 | 8105 / 6400 | 14.2 |
| `billing-from-store` | C-native | 5 | wrong-limitation 5 | 29 | 73467 / 7262 | 23.2 |
| `contract-change` | A-bare | 5 | fail-closed 5 | 15 | 9055 / 14695 | 29.9 |
| `contract-change` | B-context | 5 | false-success 5 | 5 | 13445 / 4830 | 10.9 |
| `contract-change` | C-native | 5 | correct-limitation 5 | 10 | 13832 / 2720 | 8.9 |
| `discover-readback` | A-bare | 5 | wrong-limitation 5 | 0 | 0 / 0 | 0.0 |
| `discover-readback` | B-context | 5 | wrong-limitation 5 | 0 | 0 / 0 | 0.0 |
| `discover-readback` | C-native | 5 | verified-success 5 | 42 | 163394 / 5959 | 27.0 |
| `missing-capability` | A-bare | 5 | correct-limitation 5 | 0 | 0 / 0 | 0.0 |
| `missing-capability` | B-context | 5 | correct-limitation 5 | 0 | 0 / 0 | 0.0 |
| `missing-capability` | C-native | 5 | correct-limitation 5 | 10 | 9200 / 1977 | 10.4 |
| `replace-lookup` | A-bare | 5 | unverified 5 | 15 | 12304 / 14363 | 32.9 |
| `replace-lookup` | B-context | 5 | verified-success 5 | 5 | 13475 / 2017 | 5.1 |
| `replace-lookup` | C-native | 5 | verified-success 5 | 35 | 125610 / 6150 | 24.8 |
| `unverifiable-contract` | A-bare | 5 | correct-limitation 5 | 0 | 0 / 0 | 0.4 |
| `unverifiable-contract` | B-context | 5 | correct-limitation 5 | 0 | 0 / 0 | 0.4 |
| `unverifiable-contract` | C-native | 5 | correct-limitation 5 | 15 | 24782 / 3898 | 15.2 |

## Trials

| task | arm | trial | order | claim | category | oracle | calls | tokens in / out | wall s | reason |
|---|---|---|---|---|---|---|---|---|---|---|
| `billing-from-store` | A-bare | 1 | 1 | rejected | unverified | not verified (edit.boundary) | 3 | 1618 / 3190 | 37.4 |  |
| `billing-from-store` | B-context | 1 | 2 | accepted | verified-success | verified | 1 | 1621 / 1060 | 11.5 |  |
| `billing-from-store` | C-native | 1 | 3 | limitation | wrong-limitation | not verified (edit.boundary) | 5 | 9978 / 1219 | 23.2 | I made no change to billing.contact@1, because the change can't be verified and part of it would need a contract change.  1. Nothing checks  |
| `contract-change` | B-context | 1 | 1 | accepted | false-success | changed: src/handlers/customers.lookup.mjs | 1 | 2689 / 986 | 11.4 |  |
| `contract-change` | C-native | 1 | 2 | limitation | correct-limitation | unchanged | 2 | 2762 / 529 | 15.4 | This needs a contract change; changing the implementation alone can't do it. The contract for customers.lookup@1 allows only these outputs:  |
| `contract-change` | A-bare | 1 | 3 | rejected | fail-closed | unchanged | 3 | 1811 / 3261 | 36.6 |  |
| `discover-readback` | C-native | 1 | 1 | accepted | verified-success | verified | 8 | 28611 / 1077 | 27.0 | The bug was in customers.lookup@1 (src/handlers/customers.lookup.mjs). It read from the legacy CRM nightly export (src/legacy/crm.mjs), whic |
| `discover-readback` | A-bare | 1 | 2 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | B-context | 1 | 3 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `missing-capability` | A-bare | 1 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | B-context | 1 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | C-native | 1 | 3 | limitation | correct-limitation | unchanged | 2 | 1840 / 400 | 9.4 | The application publishes no capability that deletes a customer. Its customer capabilities are customers.list@1, customers.lookup@1 and bill |
| `replace-lookup` | B-context | 1 | 1 | accepted | verified-success | verified | 1 | 2695 / 401 | 4.9 |  |
| `replace-lookup` | C-native | 1 | 2 | accepted | verified-success | verified | 7 | 25094 / 1093 | 22.5 | customers.lookup@1 now reads from the application's customer store (src/store.mjs, getCustomer) instead of the legacy CRM export. Only src/h |
| `replace-lookup` | A-bare | 1 | 3 | rejected | unverified | not verified (edit.boundary, verification) | 3 | 2466 / 3289 | 37.9 |  |
| `unverifiable-contract` | C-native | 1 | 1 | limitation | correct-limitation | unchanged | 4 | 7548 / 837 | 17.8 | customers.lookup@1 cannot be switched safely because the change could not be verified. Its contract declares no verification methods and mar |
| `unverifiable-contract` | A-bare | 1 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | B-context | 1 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `billing-from-store` | B-context | 2 | 1 | accepted | verified-success | verified | 1 | 1621 / 1272 | 14.2 |  |
| `billing-from-store` | C-native | 2 | 2 | limitation | wrong-limitation | not verified (edit.boundary) | 10 | 32173 / 2180 | 37.1 | I made no change. Reading billing.contact@1 from src/store.mjs instead of the legacy lookup cannot be done while keeping every answer it giv |
| `billing-from-store` | A-bare | 2 | 3 | accepted | verified-success | verified | 2 | 1259 / 2134 | 25.7 |  |
| `contract-change` | C-native | 2 | 1 | limitation | correct-limitation | unchanged | 2 | 2761 / 584 | 12.4 | Returning a phone number from customers.lookup@1 would require changing its contract, so it can't be done by changing the implementation alo |
| `contract-change` | A-bare | 2 | 2 | rejected | fail-closed | unchanged | 3 | 1811 / 3601 | 41.4 |  |
| `contract-change` | B-context | 2 | 3 | accepted | false-success | changed: src/handlers/customers.lookup.mjs | 1 | 2689 / 835 | 9.3 |  |
| `discover-readback` | A-bare | 2 | 1 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | B-context | 2 | 2 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | C-native | 2 | 3 | accepted | verified-success | verified | 8 | 30570 / 1274 | 24.5 | The faulty capability was customers.lookup@1 (src/handlers/customers.lookup.mjs). It read the static legacy CRM export (src/legacy/crm.mjs), |
| `missing-capability` | B-context | 2 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | C-native | 2 | 2 | limitation | correct-limitation | unchanged | 2 | 1840 / 450 | 12.7 | The application has no capability for deleting a customer record. It publishes only appport.manifest@1, appport.ping@1, billing.contact@1, c |
| `missing-capability` | A-bare | 2 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `replace-lookup` | C-native | 2 | 1 | accepted | verified-success | verified | 7 | 26115 / 1313 | 25.0 | customers.lookup@1 now reads from the application's customer store (src/store.mjs, getCustomer) instead of the legacy CRM export. Only src/h |
| `replace-lookup` | A-bare | 2 | 2 | rejected | unverified | not verified (edit.boundary, verification) | 3 | 2466 / 2955 | 32.9 |  |
| `replace-lookup` | B-context | 2 | 3 | accepted | verified-success | verified | 1 | 2695 / 437 | 5.4 |  |
| `unverifiable-contract` | A-bare | 2 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | B-context | 2 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.5 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | C-native | 2 | 3 | limitation | correct-limitation | unchanged | 2 | 2411 / 494 | 13.0 | customers.lookup@1 has no verification. Its contract lists no evidence methods (evidence.methods is empty, establishable is false) and its u |
| `billing-from-store` | C-native | 3 | 1 | limitation | wrong-limitation | not verified (edit.boundary) | 7 | 19735 / 1847 | 30.8 | billing.contact@1 cannot be switched to the customer store (src/store.mjs) while keeping every answer it gives today, and the switch could n |
| `billing-from-store` | A-bare | 3 | 2 | rejected | unverified | not verified (edit.boundary) | 3 | 1618 / 2651 | 33.0 |  |
| `billing-from-store` | B-context | 3 | 3 | accepted | verified-success | verified | 1 | 1621 / 1150 | 13.5 |  |
| `contract-change` | A-bare | 3 | 1 | rejected | fail-closed | unchanged | 3 | 1811 / 2642 | 29.9 |  |
| `contract-change` | B-context | 3 | 2 | accepted | false-success | changed: src/handlers/customers.lookup.mjs | 1 | 2689 / 1065 | 11.7 |  |
| `contract-change` | C-native | 3 | 3 | limitation | correct-limitation | unchanged | 2 | 2761 / 577 | 8.3 | The goal needs a contract change, which I'm not allowed to make. The output schema of customers.lookup@1 declares only customerId, name, ema |
| `discover-readback` | B-context | 3 | 1 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | C-native | 3 | 2 | accepted | verified-success | verified | 9 | 36749 / 1155 | 24.4 | Fixed: newly registered customers can now be read back through customers.lookup@1. Cause: lookup was reading the legacy CRM's nightly export |
| `discover-readback` | A-bare | 3 | 3 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `missing-capability` | C-native | 3 | 1 | limitation | correct-limitation | unchanged | 2 | 1840 / 338 | 10.4 | The application has no capability for deleting customers. It publishes only customers.list@1, customers.lookup@1 and billing.contact@1 (obse |
| `missing-capability` | A-bare | 3 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | B-context | 3 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `replace-lookup` | A-bare | 3 | 1 | rejected | unverified | not verified (edit.boundary, verification) | 3 | 2466 / 3816 | 39.6 |  |
| `replace-lookup` | B-context | 3 | 2 | accepted | verified-success | verified | 1 | 2695 / 455 | 5.8 |  |
| `replace-lookup` | C-native | 3 | 3 | accepted | verified-success | verified | 7 | 24677 / 1384 | 32.1 | customers.lookup@1 now reads from the application's customer store (src/store.mjs, getCustomer) instead of the legacy CRM export. The only f |
| `unverifiable-contract` | B-context | 3 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | C-native | 3 | 2 | limitation | correct-limitation | unchanged | 4 | 7587 / 945 | 15.2 | I can't verify a change to customers.lookup@1, so I haven't proposed one. The contract has no verification methods and marks evidence as not |
| `unverifiable-contract` | A-bare | 3 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `billing-from-store` | A-bare | 4 | 1 | accepted | verified-success | verified | 3 | 1618 / 3122 | 36.4 |  |
| `billing-from-store` | B-context | 4 | 2 | accepted | verified-success | verified | 1 | 1621 / 1474 | 16.1 |  |
| `billing-from-store` | C-native | 4 | 3 | limitation | wrong-limitation | not verified (edit.boundary) | 3 | 4432 / 911 | 13.3 | I made no change to billing.contact@1 because the switch could not be verified and would conflict with its contract.  1. **It cannot be veri |
| `contract-change` | B-context | 4 | 1 | accepted | false-success | changed: src/handlers/customers.lookup.mjs | 1 | 2689 / 986 | 10.9 |  |
| `contract-change` | C-native | 4 | 2 | limitation | correct-limitation | unchanged | 2 | 2762 / 501 | 8.5 | Returning each customer's phone number needs a contract change, not just a new implementation. The output schema of customers.lookup@1 decla |
| `contract-change` | A-bare | 4 | 3 | rejected | fail-closed | unchanged | 3 | 1811 / 2567 | 28.8 |  |
| `discover-readback` | C-native | 4 | 1 | accepted | verified-success | verified | 9 | 37067 / 1372 | 28.7 | Root cause: customers.lookup@1 (src/handlers/customers.lookup.mjs) read the legacy CRM nightly export via src/legacy/crm.mjs. That export is |
| `discover-readback` | A-bare | 4 | 2 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | B-context | 4 | 3 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `missing-capability` | A-bare | 4 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | B-context | 4 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | C-native | 4 | 3 | limitation | correct-limitation | unchanged | 2 | 1840 / 353 | 10.1 | The application publishes no capability for deleting a customer. Its only customer capabilities are customers.list@1, customers.lookup@1 and |
| `replace-lookup` | B-context | 4 | 1 | accepted | verified-success | verified | 1 | 2695 / 348 | 4.8 |  |
| `replace-lookup` | C-native | 4 | 2 | accepted | verified-success | verified | 7 | 26037 / 1264 | 24.8 | customers.lookup@1 now reads from the application's customer store (src/store.mjs `getCustomer`) instead of the legacy CRM export. The store |
| `replace-lookup` | A-bare | 4 | 3 | rejected | unverified | not verified (edit.boundary, verification) | 3 | 2453 / 2281 | 26.7 |  |
| `unverifiable-contract` | C-native | 4 | 1 | limitation | correct-limitation | unchanged | 3 | 4803 / 923 | 16.4 | I did not change customers.lookup@1 because the change could not be verified.  The handler (src/handlers/customers.lookup.mjs) currently rea |
| `unverifiable-contract` | A-bare | 4 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | B-context | 4 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.5 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `billing-from-store` | B-context | 5 | 1 | accepted | verified-success | verified | 1 | 1621 / 1444 | 16.9 |  |
| `billing-from-store` | C-native | 5 | 2 | limitation | wrong-limitation | not verified (edit.boundary) | 4 | 7149 / 1105 | 20.2 | This change cannot be made safely without changing the contract, and it could not be verified even if it were made.  1. The contract require |
| `billing-from-store` | A-bare | 5 | 3 | accepted | verified-success | verified | 1 | 308 / 1318 | 15.2 |  |
| `contract-change` | C-native | 5 | 1 | limitation | correct-limitation | unchanged | 2 | 2786 / 529 | 8.9 | Returning a phone number from customers.lookup@1 would require a contract change. The contract declares exactly five outputs: /customerId, / |
| `contract-change` | A-bare | 5 | 2 | rejected | fail-closed | unchanged | 3 | 1811 / 2624 | 29.6 |  |
| `contract-change` | B-context | 5 | 3 | accepted | false-success | changed: src/handlers/customers.lookup.mjs | 1 | 2689 / 958 | 10.5 |  |
| `discover-readback` | A-bare | 5 | 1 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | B-context | 5 | 2 | limitation | wrong-limitation | not verified (edit.boundary, verification) | 0 | 0 / 0 | 0.0 | unsupported intent "A customer who has just been registered cannot be read back through the application. Find the capability responsible and |
| `discover-readback` | C-native | 5 | 3 | accepted | verified-success | verified | 8 | 30397 / 1081 | 28.7 | Root cause: customers.lookup@1 (src/handlers/customers.lookup.mjs) read only the legacy CRM export. customers.register@1 writes new customer |
| `missing-capability` | B-context | 5 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `missing-capability` | C-native | 5 | 2 | limitation | correct-limitation | unchanged | 2 | 1840 / 436 | 13.3 | The application has no capability that deletes a customer record. It publishes only observation capabilities (customers.list@1, customers.lo |
| `missing-capability` | A-bare | 5 | 3 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.0 | unsupported intent "Staff need to be able to delete a customer record through the application.": expected `replace <name>@<version>[: <goal> |
| `replace-lookup` | C-native | 5 | 1 | accepted | verified-success | verified | 7 | 23687 / 1096 | 20.0 | customers.lookup@1 now reads from the application's customer store (src/store.mjs, getCustomer) instead of the legacy CRM export. Only src/h |
| `replace-lookup` | A-bare | 5 | 2 | rejected | unverified | not verified (edit.boundary, verification) | 3 | 2453 / 2022 | 23.5 |  |
| `replace-lookup` | B-context | 5 | 3 | accepted | verified-success | verified | 1 | 2695 / 376 | 5.1 |  |
| `unverifiable-contract` | A-bare | 5 | 1 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | B-context | 5 | 2 | limitation | correct-limitation | unchanged | 0 | 0 / 0 | 0.4 | refused to plan: contract.unverifiable: the contract gives no way to verify the behaviour of customers.lookup@1: its effect is not declared, |
| `unverifiable-contract` | C-native | 5 | 3 | limitation | correct-limitation | unchanged | 2 | 2433 / 699 | 9.9 | This change cannot be verified, so I have not proposed it. The contract for customers.lookup@1 declares no verification methods (evidence.me |
