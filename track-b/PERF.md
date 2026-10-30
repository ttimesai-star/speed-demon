# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | Rust: dictionary of the query terms only, parallel tokenization of the mapped corpus into per-thread posting lists, BOOL strictly left to right by merging sorted lists, PHRASE as the intersection of its terms' lists (plus a run-length list for terms a phrase repeats) checked by re-tokenizing each candidate document (commit 4130ed7) | run-de4befb1b622 | 1.296 | 232.5 | 46.52 |
| 2 | `perf` on step 1: dictionary probes 27 % (a 16 MB table), the byte-wise FNV hash and byte classification ~22 %, linear merge walks over long lists 11 %, candidate re-tokenization 13 %. Galloping intersection and difference (Linux A/B 5529.9 → 4971.3 ms, −10 %); SWAR tokenizer: ASCII-lowercase, token-byte mask and the hash eight bytes at a time, and a dictionary of tag-and-id words sized to 4x the term count (→ 4236.6 ms, −15 %). Phrases matched inside the corpus pass: each phrase has a trigger term (its longest term, a cheap proxy for a rare one; any choice is correct), and at the end of each document the token-id buffer is compared around every occurrence of a trigger; no candidates, no second tokenization, no run-length lists (A/B 4718.3 → 3481.1 ms, −26 %) (commit f5556c8) | run-aa5535294b39 | 0.892 | 232.6 | 50.56 |
| 3 | Frequent terms as bitsets: a thread's list turns into a bitset of its document range once it passes range / 32, the merged set is a bitset of all documents when any thread made one; BOOL over every mix of lists and bitsets; the merge runs in parallel by term range (A/B 3384.4 → 3022.2 ms, −11 %). M did not move (harness, 12 threads forced: 242 → 251 MB) and S grew 332 → 460 (commit 2b20529, which also sets `PR_SET_THP_DISABLE`: no effect on M here) | run-9741fe7a62ad | 0.753 | 252.0 | 50.99 |
| 4 | Where M went (PC, 12 threads): per-thread lists 71 MB of capacity for 48 MB of ids, 29 MB of per-thread set headers (12 × 75,747 terms), 11 MB of thread bitsets, and freed small vectors that malloc keeps. Now each thread writes one flat stream of term ids per document plus counts; exact sizes come from the counts, and each thread lays its stream into the final lists (or atomic ORs into the final bitsets) at its own offsets. A list that meets a bitset in BOOL becomes a bitset, four arms instead of twelve. A/B 3509.2 → 3143.5 ms (−10 %); harness with 12 threads forced: M 251 → 176 MB (commit 80f4bd0, pushed as 38ff772) | run-18079cc0a31a | 0.655 | 176.6 | 53.91 |
| 5 | SWAR search for each document's TAB and LF (the byte-wise `position` was one more full pass over 254 MB). PC 429 → 348 ms; Linux A/B −2 % (commit 754e203, pushed as cad0914) | run-02ffeebb7146 | 0.611 | 176.4 | 54.60 |
| 6 | M at the peak (PC, 12 threads): thread streams 60 MB resident, thread bitsets 11 MB, per-thread arrays 10 MB, final lists and bitsets 59 MB, all alive during the layout. Each thread now hands the laid-out part of its stream back to the kernel (`madvise(MADV_DONTNEED)` every 2^18 entries), so the streams shrink while the final lists fill (commit bc69acb, run-d66ca6878bb3: M 176.5 MB, unchanged; T 0.629 s). Then each thread's stream reserved for the worst case so it never moves (commit cb2f844, run-4b5c83edca37: M 175.5 MB, unchanged). Neither moved the peak, so the peak is not where the PC accounting puts it; both reverted, the code is step 5 again | run-4b5c83edca37 | 0.632 | 175.5 | 54.11 |
| 7 | Phrase checks measured on the PC: with them switched off the corpus pass is 211 ms instead of 270 ms. The trigger was the longest term, often a common word. Now a parallel pre-pass counts documents per term on the first 1/64 of every chunk, and the trigger is the rarest term by that count, then the longest. PC 348 → 293 ms (−16 %) (commit a1f4092, pushed as 7dc1c88) | run-a9d10c49aa96 | 0.4947 | 179.6 | 56.71 |
| 8 | Sizes and per-thread offsets of the final sets in parallel by term range; BOOL without bitset copies: AND with a list keeps the list side, OR and NOT on a bitset work in place, only list OR bitset copies (the four-arm version cloned a 31 KB bitset per operation) (commits 501e54c, 3931f5e) | run-207db152c61e | 0.4796 | 184.6 | 56.88 |
| 9 | Tokenizer one token ahead: the next token is found and hashed and its dictionary slot prefetched before the current one is resolved. PC 287 → 270 ms (commit 2437d05, pushed as eb74b43) | run-7ab17109b510 | 0.4611 | 184.0 | 57.21 |
| 10 | S: unix-only `load` (the PC build swaps in its own), a shorter `par()`; plus a `malloc_trim(0)` before the answers as a test of whether the peak is freed prep memory kept by malloc: M 184.8 MB, unchanged, so it is not (commit 1309be5; the trim is removed again in step 11) | run-9bd4481a6298 | 0.445 | 184.8 | 57.61 |
| 11 | Byte search 32 bytes at a time with AVX2 (x86-64-v3) for each document's TAB and LF; a multiplicative hasher for the term map and one word buffer for all query lines (the query parse is serial). PC 270 → 264 ms | | | | |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 and seed 3 sample and seed 1 tiny (hashes of
the reference run locally on the same generated data), and the spec's worked example byte for byte.

## PC phase split (step 2 before the in-pass phrases, 8 threads, seed 1 full)

Queries parsed 18 ms, corpus pass ~255 ms (with the Windows file read), list concatenation ~55 ms, answers
~265 ms, of which BOOL ~100 ms and PHRASE ~160 ms (measured with BOOL-only and PHRASE-only query files on the
same corpus). The PHRASE share was the re-tokenization of candidates, more bytes than the corpus itself.

## Tried and reverted

- **Bloom filter in front of the dictionary** (128 KB, one bit per term hash; 9e6ad48): PC −20 % (429 → 342 ms),
  Linux A/B +2 % (2602.7 vs 2654.6 ms), board 0.6556 s vs 0.6525 s with S +11 (run-9aa73a6479ad, 53.82 points).
  Reverted: the cache effect does not carry from one machine to another.
- **PGO** (3,000 documents and 2,000 queries of the seed-2 sample): board 0.6456 s vs 0.6115 s (run-a6a09a082994). Reverted.
- **One 12-byte state record per term per thread and a 75 KB trigger-flag array** instead of three 300 KB arrays and the trigger lists' headers: PC neutral, board 0.612 s vs 0.6115 s with M +7 MB (run-a9acfad6fcf0). Reverted.
