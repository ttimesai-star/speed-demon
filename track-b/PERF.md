# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | Rust: dictionary of the query terms only, parallel tokenization of the mapped corpus into per-thread posting lists, BOOL strictly left to right by merging sorted lists, PHRASE as the intersection of its terms' lists (plus a run-length list for terms a phrase repeats) checked by re-tokenizing each candidate document | | | | |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 and seed 3 sample and seed 1 tiny (hashes of
the reference run locally on the same generated data), and the spec's worked example byte for byte.
