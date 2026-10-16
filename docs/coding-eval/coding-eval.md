# Coding-agent evaluation (no AppPort)

- model: `anthropic:claude-opus-5-5 via https://api.anthropic.com/v1/messages`
- attempts per trial: up to 3
- run: 2026-10-16T00:09:04.400Z → 2026-10-16T00:14:16.503Z

Success = accepted by the project's own tests **and** passing hidden acceptance tests the agent never saw.

| task | arm | trials | success | accepted on 1st attempt | false accepts | rejected | refused | errors | model calls | tokens in / out | test runs | rollbacks | boundary violations | test edits attempted | stayed in intended files | mean wall time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `bugfix-invoice-discount` | observed | 10 | 10 (100%) | 10 | 0 | 0 | 0 | 0 | 10 | 21709 / 5280 | 20 | 0 | 0 | 0 | 10/10 | 5.7 s |
| `bugfix-invoice-discount` | minimal | 10 | 10 (100%) | 10 | 0 | 0 | 0 | 0 | 10 | 5610 / 3906 | 20 | 0 | 0 | 0 | 10/10 | 5.8 s |
| `feature-slug-max-length` | observed | 10 | 10 (100%) | 10 | 0 | 0 | 0 | 0 | 10 | 24940 / 9188 | 20 | 0 | 0 | 0 | 10/10 | 9.3 s |
| `feature-slug-max-length` | minimal | 10 | 10 (100%) | 10 | 0 | 0 | 0 | 0 | 10 | 4420 / 9575 | 20 | 0 | 0 | 0 | 10/10 | 9.8 s |
| **all** | observed | 20 | 20 (100%) | 20 | 0 | 0 | 0 | 0 | 20 | 46649 / 14468 | 40 | 0 | 0 | 0 | 20/20 | 7.5 s |
| **all** | minimal | 20 | 20 (100%) | 20 | 0 | 0 | 0 | 0 | 20 | 10030 / 13481 | 40 | 0 | 0 | 0 | 20/20 | 7.8 s |

## Trials

| task | arm | trial | outcome | hidden tests | attempts | tokens in / out | test runs | wall time |
|---|---|---|---|---|---|---|---|---|
| `bugfix-invoice-discount` | observed | 1 | accepted | pass | 1 | 2171 / 522 | 2 | 5.5 s |
| `bugfix-invoice-discount` | observed | 2 | accepted | pass | 1 | 2170 / 522 | 2 | 5.5 s |
| `bugfix-invoice-discount` | observed | 3 | accepted | pass | 1 | 2171 / 536 | 2 | 5.4 s |
| `bugfix-invoice-discount` | observed | 4 | accepted | pass | 1 | 2171 / 459 | 2 | 4.9 s |
| `bugfix-invoice-discount` | observed | 5 | accepted | pass | 1 | 2171 / 475 | 2 | 5.6 s |
| `bugfix-invoice-discount` | observed | 6 | accepted | pass | 1 | 2171 / 491 | 2 | 5.9 s |
| `bugfix-invoice-discount` | observed | 7 | accepted | pass | 1 | 2171 / 628 | 2 | 6.9 s |
| `bugfix-invoice-discount` | observed | 8 | accepted | pass | 1 | 2171 / 532 | 2 | 5.6 s |
| `bugfix-invoice-discount` | observed | 9 | accepted | pass | 1 | 2171 / 588 | 2 | 6.3 s |
| `bugfix-invoice-discount` | observed | 10 | accepted | pass | 1 | 2171 / 527 | 2 | 5.6 s |
| `bugfix-invoice-discount` | minimal | 1 | accepted | pass | 1 | 561 / 388 | 2 | 5.3 s |
| `bugfix-invoice-discount` | minimal | 2 | accepted | pass | 1 | 561 / 388 | 2 | 4.7 s |
| `bugfix-invoice-discount` | minimal | 3 | accepted | pass | 1 | 561 / 388 | 2 | 6.9 s |
| `bugfix-invoice-discount` | minimal | 4 | accepted | pass | 1 | 561 / 394 | 2 | 6.8 s |
| `bugfix-invoice-discount` | minimal | 5 | accepted | pass | 1 | 561 / 390 | 2 | 8.8 s |
| `bugfix-invoice-discount` | minimal | 6 | accepted | pass | 1 | 561 / 387 | 2 | 5.3 s |
| `bugfix-invoice-discount` | minimal | 7 | accepted | pass | 1 | 561 / 392 | 2 | 4.9 s |
| `bugfix-invoice-discount` | minimal | 8 | accepted | pass | 1 | 561 / 397 | 2 | 4.8 s |
| `bugfix-invoice-discount` | minimal | 9 | accepted | pass | 1 | 561 / 394 | 2 | 5.7 s |
| `bugfix-invoice-discount` | minimal | 10 | accepted | pass | 1 | 561 / 388 | 2 | 5.1 s |
| `feature-slug-max-length` | observed | 1 | accepted | pass | 1 | 2494 / 882 | 2 | 8.4 s |
| `feature-slug-max-length` | observed | 2 | accepted | pass | 1 | 2494 / 1151 | 2 | 11.8 s |
| `feature-slug-max-length` | observed | 3 | accepted | pass | 1 | 2494 / 926 | 2 | 9.0 s |
| `feature-slug-max-length` | observed | 4 | accepted | pass | 1 | 2494 / 797 | 2 | 8.0 s |
| `feature-slug-max-length` | observed | 5 | accepted | pass | 1 | 2494 / 853 | 2 | 9.6 s |
| `feature-slug-max-length` | observed | 6 | accepted | pass | 1 | 2494 / 898 | 2 | 9.0 s |
| `feature-slug-max-length` | observed | 7 | accepted | pass | 1 | 2494 / 1143 | 2 | 11.4 s |
| `feature-slug-max-length` | observed | 8 | accepted | pass | 1 | 2494 / 822 | 2 | 8.3 s |
| `feature-slug-max-length` | observed | 9 | accepted | pass | 1 | 2494 / 837 | 2 | 8.6 s |
| `feature-slug-max-length` | observed | 10 | accepted | pass | 1 | 2494 / 879 | 2 | 8.4 s |
| `feature-slug-max-length` | minimal | 1 | accepted | pass | 1 | 442 / 1035 | 2 | 10.8 s |
| `feature-slug-max-length` | minimal | 2 | accepted | pass | 1 | 442 / 978 | 2 | 10.1 s |
| `feature-slug-max-length` | minimal | 3 | accepted | pass | 1 | 442 / 856 | 2 | 8.4 s |
| `feature-slug-max-length` | minimal | 4 | accepted | pass | 1 | 442 / 941 | 2 | 9.8 s |
| `feature-slug-max-length` | minimal | 5 | accepted | pass | 1 | 442 / 1052 | 2 | 10.9 s |
| `feature-slug-max-length` | minimal | 6 | accepted | pass | 1 | 442 / 1144 | 2 | 11.1 s |
| `feature-slug-max-length` | minimal | 7 | accepted | pass | 1 | 442 / 863 | 2 | 9.3 s |
| `feature-slug-max-length` | minimal | 8 | accepted | pass | 1 | 442 / 835 | 2 | 8.6 s |
| `feature-slug-max-length` | minimal | 9 | accepted | pass | 1 | 442 / 877 | 2 | 8.9 s |
| `feature-slug-max-length` | minimal | 10 | accepted | pass | 1 | 442 / 994 | 2 | 9.5 s |
