# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | Rust: dictionary of the query terms only, parallel tokenization of the mapped corpus into per-thread posting lists, BOOL strictly left to right by merging sorted lists, PHRASE as the intersection of its terms' lists (plus a run-length list for terms a phrase repeats) checked by re-tokenizing each candidate document (commit 4130ed7) | run-de4befb1b622 | 1.296 | 232.5 | 46.52 |
| 2 | `perf` on step 1: dictionary probes 27 % (a 16 MB table), the byte-wise FNV hash and byte classification ~22 %, linear merge walks over long lists 11 %, candidate re-tokenization 13 %. Galloping intersection and difference (Linux A/B 5529.9 → 4971.3 ms, −10 %); SWAR tokenizer: ASCII-lowercase, token-byte mask and the hash eight bytes at a time, and a dictionary of tag-and-id words sized to 4x the term count (→ 4236.6 ms, −15 %). Phrases matched inside the corpus pass: each phrase has a trigger term (its longest term, a cheap proxy for a rare one; any choice is correct), and at the end of each document the token-id buffer is compared around every occurrence of a trigger; no candidates, no second tokenization, no run-length lists (A/B 4718.3 → 3481.1 ms, −26 %) (commit f5556c8) | run-aa5535294b39 | 0.892 | 232.6 | 50.56 |
| 3 | Frequent terms as bitsets: a thread's list turns into a bitset of its document range once it passes range / 32, the merged set is a bitset of all documents when any thread made one; BOOL over every mix of lists and bitsets; the merge runs in parallel by term range (A/B 3384.4 → 3022.2 ms, −11 %). M did not move (harness, 12 threads forced: 242 → 251 MB) and S grew 332 → 460 (commit 2b20529, which also sets `PR_SET_THP_DISABLE`: no effect on M here) | run-9741fe7a62ad | 0.753 | 252.0 | 50.99 |
| 4 | Where M went (PC, 12 threads): per-thread lists 71 MB of capacity for 48 MB of ids, 29 MB of per-thread set headers (12 × 75,747 terms), 11 MB of thread bitsets, and freed small vectors that malloc keeps. Now each thread writes one flat stream of term ids per document plus counts; exact sizes come from the counts, and each thread lays its stream into the final lists (or atomic ORs into the final bitsets) at its own offsets. A list that meets a bitset in BOOL becomes a bitset, four arms instead of twelve. A/B 3509.2 → 3143.5 ms (−10 %); harness with 12 threads forced: M 251 → 176 MB (commit 80f4bd0, pushed as 38ff772) | run-18079cc0a31a | 0.655 | 176.6 | 53.91 |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 and seed 3 sample and seed 1 tiny (hashes of
the reference run locally on the same generated data), and the spec's worked example byte for byte.

## PC phase split (step 2 before the in-pass phrases, 8 threads, seed 1 full)

Queries parsed 18 ms, corpus pass ~255 ms (with the Windows file read), list concatenation ~55 ms, answers
~265 ms, of which BOOL ~100 ms and PHRASE ~160 ms (measured with BOOL-only and PHRASE-only query files on the
same corpus). The PHRASE share was the re-tokenization of candidates, more bytes than the corpus itself.
