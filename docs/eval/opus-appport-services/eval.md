# Closed-loop evaluation

- intent: `replace documents.open@1: return a copy of the stored document instead of the live record`
- project: `target/closed-loop/services-pristine`
- model: `anthropic:claude-opus-5-5 via https://api.anthropic.com/v1/messages`
- attempts per trial: up to 3
- run: 2026-10-15T23:22:52.652Z → 2026-10-15T23:28:16.455Z

## Outcomes

| arm | trials | accepted on 1st attempt | accepted after retry | rejected (all attempts) | errors | acceptance rate | mean attempts to accept |
|---|---|---|---|---|---|---|---|
| knowledge | 10 | 10 (100%) | 0 | 0 | 0 | 100% | 1.00 |
| bare | 10 | 10 (100%) | 0 | 0 | 0 | 100% | 1.00 |

## Cost and behaviour

| arm | model calls | tokens in / out | model calls per accepted | tokens per accepted | rejected attempts | rollbacks | contract edits | boundary violations | unusable responses | mean wall time |
|---|---|---|---|---|---|---|---|---|---|---|
| knowledge | 10 | 29040 / 17347 | 1.00 | 4639 | 0 | 0 | 0 | 0 | 0 | 16.1 s |
| bare | 10 | 18190 / 17207 | 1.00 | 3540 | 0 | 0 | 0 | 0 | 0 | 16.3 s |

## Why proposals were rejected

| arm | check | rejections |
|---|---|---|

## Trials

| arm | trial | outcome | attempts | rejected by | tokens in / out | wall time |
|---|---|---|---|---|---|---|
| knowledge | 1 | accepted | 1 | – | 2904 / 1721 | 16.0 s |
| knowledge | 2 | accepted | 1 | – | 2904 / 1733 | 16.1 s |
| knowledge | 3 | accepted | 1 | – | 2904 / 1736 | 16.0 s |
| knowledge | 4 | accepted | 1 | – | 2904 / 1733 | 15.9 s |
| knowledge | 5 | accepted | 1 | – | 2904 / 1711 | 16.1 s |
| knowledge | 6 | accepted | 1 | – | 2904 / 1764 | 16.4 s |
| knowledge | 7 | accepted | 1 | – | 2904 / 1733 | 16.1 s |
| knowledge | 8 | accepted | 1 | – | 2904 / 1711 | 15.8 s |
| knowledge | 9 | accepted | 1 | – | 2904 / 1758 | 16.2 s |
| knowledge | 10 | accepted | 1 | – | 2904 / 1747 | 16.2 s |
| bare | 1 | accepted | 1 | – | 1819 / 1710 | 16.2 s |
| bare | 2 | accepted | 1 | – | 1819 / 1740 | 16.2 s |
| bare | 3 | accepted | 1 | – | 1819 / 1710 | 16.1 s |
| bare | 4 | accepted | 1 | – | 1819 / 1691 | 16.6 s |
| bare | 5 | accepted | 1 | – | 1819 / 1711 | 16.1 s |
| bare | 6 | accepted | 1 | – | 1819 / 1710 | 16.1 s |
| bare | 7 | accepted | 1 | – | 1819 / 1750 | 16.4 s |
| bare | 8 | accepted | 1 | – | 1819 / 1764 | 16.8 s |
| bare | 9 | accepted | 1 | – | 1819 / 1710 | 16.1 s |
| bare | 10 | accepted | 1 | – | 1819 / 1711 | 16.2 s |
