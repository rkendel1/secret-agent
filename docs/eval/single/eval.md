# Closed-loop evaluation

- intent: `replace documents.open@1: return a copy of the stored document instead of the live record`
- project: `target/closed-loop/services-pristine`
- model: `anthropic:claude-opus-5-5 via https://api.anthropic.com/v1/messages`
- attempts per trial: up to 3
- run: 2026-10-15T23:22:21.648Z → 2026-10-15T23:22:37.697Z

## Outcomes

| arm | trials | accepted on 1st attempt | accepted after retry | rejected (all attempts) | errors | acceptance rate | mean attempts to accept |
|---|---|---|---|---|---|---|---|
| knowledge | 1 | 1 (100%) | 0 | 0 | 0 | 100% | 1.00 |

## Cost and behaviour

| arm | model calls | tokens in / out | model calls per accepted | tokens per accepted | rejected attempts | rollbacks | contract edits | boundary violations | unusable responses | mean wall time |
|---|---|---|---|---|---|---|---|---|---|---|
| knowledge | 1 | 2904 / 1733 | 1.00 | 4637 | 0 | 0 | 0 | 0 | 0 | 16.0 s |

## Why proposals were rejected

| arm | check | rejections |
|---|---|---|

## Trials

| arm | trial | outcome | attempts | rejected by | tokens in / out | wall time |
|---|---|---|---|---|---|---|
| knowledge | 1 | accepted | 1 | – | 2904 / 1733 | 16.0 s |
