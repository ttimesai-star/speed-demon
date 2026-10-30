# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | Rust: dictionary of the query terms only, parallel tokenization of the mapped corpus into per-thread posting lists, BOOL strictly left to right by merging sorted lists, PHRASE as the intersection of its terms' lists (plus a run-length list for terms a phrase repeats) checked by re-tokenizing each candidate document (commit 4130ed7) | run-de4befb1b622 | 1.296 | 232.5 | 46.52 |
| 2 | `perf` on step 1: dictionary probes 27 % (a 16 MB table), the byte-wise FNV hash and byte classification ~22 %, linear merge walks over long lists 11 %, candidate re-tokenization 13 %. Galloping intersection and difference (Linux A/B 5529.9 → 4971.3 ms, −10 %); SWAR tokenizer: ASCII-lowercase, token-byte mask and the hash eight bytes at a time, and a dictionary of tag-and-id words sized to 4x the term count (→ 4236.6 ms, −15 %). Phrases matched inside the corpus pass: each phrase has a trigger term (its longest term, a cheap proxy for a rare one; any choice is correct), and at the end of each document the token-id buffer is compared around every occurrence of a trigger; no candidates, no second tokenization, no run-length lists (A/B 4718.3 → 3481.1 ms, −26 %) | | | | |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 and seed 3 sample and seed 1 tiny (hashes of
the reference run locally on the same generated data), and the spec's worked example byte for byte.

## PC phase split (step 2 before the in-pass phrases, 8 threads, seed 1 full)

Queries parsed 18 ms, corpus pass ~255 ms (with the Windows file read), list concatenation ~55 ms, answers
~265 ms, of which BOOL ~100 ms and PHRASE ~160 ms (measured with BOOL-only and PHRASE-only query files on the
same corpus). The PHRASE share was the re-tokenization of candidates, more bytes than the corpus itself.
