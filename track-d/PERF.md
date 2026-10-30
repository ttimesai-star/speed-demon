# Track D: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 72.69 | 893 | 0 |
| 1 | Rust: mmap, two-pass parallel CSR, per-query Dijkstra (binary heap, early stop) on all threads, `-1` at once for targets with no in-edges and sources with no out-edges (commit 4296630) | run-fb3879b67c30 | 0.909 | 234.8 | 49.16 |
| 2 | Parallel BFS renumbering (one BFS per thread, CAS claims), CSR rebuilt in the new order; `u32` distances and one-word heap entries, `u64` hash-map fallback for sums ≥ 2^32 − 1. PC (8 threads): queries 345 → 130 ms, whole run ~400 → ~220 ms (commit 2610429) | run-98ac9ddadc83 | 0.411 | 210.7 | 56.62 |
| 3 | SWAR number parse (A/B −1.7 %), the first CSR freed before the queries; bidirectional Dijkstra on a reverse CSR, both halves `u32` (Linux A/B 2011 → 1227 ms, −39 %) (commit bd27f6e) | run-83b2399db32d | 0.262 | 289.2 | 59.28 |
| 4 | CSR without atomics: each thread cuts its edges by tail range, then thread k counts, sums and fills range k (the `fetch_add` on a random cursor was 28 % of all samples in `perf`: locked RMWs serialise the cache misses). Same for the reverse CSR. Linux A/B 1368 → 1095 ms (−20 %). Meeting check at settle time instead of on every relaxed edge (A/B 955.6 → 937.7 ms, −1.9 %); the id map freed after reading the queries (commit 006c614) | run-6b5cacfa95d5 | 0.238 | 275.1 | 60.44 |
| 5 | M: per-search open-addressing tables (2^18 slots of `(node + 1) << 32 \| dist` per half, reset by the list of touched slots, a search that would fill half the table goes to the `u64` path) instead of two dense `u32` arrays per thread; reverse CSR by two scans of the forward edges per head range instead of a 60 MB edge copy. Harness with 12 threads forced: M 274 → 158 MB; Linux A/B 901.9 → 941.7 ms (+4.4 %). By the K formula: −0.5 points for T, +2.4 for M (commit 1577ee3) | run-a741c2a56a3e | 0.514 | 157.7 | 54.03 — **reverted**: the board ran it 2.2x slower than step 4, the CI runner only 4 % slower. The code is back to step 4 |
| 5b | Distance arrays zeroed by calloc with `!dist` stored (no fill pass), and queries in source-sorted blocks so that a thread's searches stay in a few regions of the id order. Harness, 12 threads: M 274 → 262 MB only (the BFS order is not spatial: sorted sources sit on the same BFS rings at all angles). Blocks dropped; the zeroed arrays alone: Linux A/B 1024.5 → 1005.4 ms (−1.9 %), board 0.2485 s, no better than step 4 (commit f241505, run-1380f6286219, 60.03 points); reverted | run-1380f6286219 | 0.2485 | 272.1 | 60.03 |
| 6 | Serial work removed: the sweep for nodes no BFS reached did a CAS on every one of the 1.74 M nodes in one thread; now a plain load first. New ids and new-order degrees in parallel by position. PC 148 → 125 ms (−16 %); Linux A/B (2 vCPU, where serial work weighs less) 823.9 → 810.1 ms (commit a7178e6, pushed as 9e6ad48) | run-fb13141bf3c0 | 0.214 | 296.4 | 61.21 |
| 7 | Step 6 kept the old offsets, the BFS order and the degrees alive into the query phase (+21 MB): the reference names shadowed the vectors, so `drop` freed the references. Fixed (harness, 12 threads: 282 MB) (commit e80f39d, pushed as cad0914) | run-809d9adc5d18 | 0.2135 | 282.4 | 61.45 |
| — | PGO (training on the seed-2 sample): board 0.2125 s, neutral; reverted | run-550d330bd124 | 0.2125 | 281.9 | 61.50 |
| 8 | No fresh pages for the reverse CSR: its buckets reuse the forward pass's buckets and its edge array reuses the old (pre-renumbering) edge array, written slot by slot without a zero fill. PC 124 → 121 ms (commit da82400, pushed as cb2f844) | run-44ed9514eec5 | 0.2047 | 282.7 | 61.82 |

## PC measurements (Ryzen 7 9800X3D, 8 threads, Windows build, `fs::read` instead of mmap)

| Variant | Parse + CSR | Renumber | Queries | Total |
|---|---|---|---|---|
| Step 1 | 55 ms | — | 345 ms | ~400 ms |
| + single-thread BFS renumbering | 55 ms | 63 ms | 225 ms | ~350 ms |
| + bidirectional Dijkstra (reverse CSR) | 55 ms | 63 ms + 11 ms | 230–265 ms | ~400 ms — not taken |
| + parallel BFS (step 2 without u32) | 55 ms | 35 ms | 200 ms | ~300 ms |
| + `u32` distances (step 2) | 47 ms | 28 ms | 130 ms | ~210–250 ms |
| + 4-ary heap (on step 7) | | | | 127.6 vs 124.7 ms, +2 % — not taken |
| + radix heap | 47 ms | 28 ms | 140 ms | 267 vs 255 ms median of 7 — noise, not taken; Linux A/B on top of the SWAR parse: 2090.7 vs 1972.4 ms, +6 % — not taken |

The board runs ~2.2x slower than the PC on step 1 (0.909 s vs ~0.4 s): the board's i5-13500 has 24 MB of L3
against the PC's 96 MB, and Dijkstra on randomly labelled nodes is bound by cache misses. That is why
locality (step 2) came first.

Correctness, every step: seed 1 sample and full (published hashes), seed 2 sample and full and seed 1 tiny
(reference run locally on the same data), the spec's worked example.

## Linux phase profile (step 3, 2 vCPU runner, `perf record`)

Parse + degree count 177 ms, edge placement 420 ms, BFS 115 ms, renumbering 100 ms, reverse CSR 65 ms,
queries 480 ms. `perf`: `Side::step` 33 %, the edge-placement closure 29 % (all of it on the
`cur[u].fetch_add` line), parse 12 %. Step 4 removes every atomic RMW from the CSR builds.
