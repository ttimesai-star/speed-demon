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
| — | Software prefetch of the neighbours' labels before relaxing, and on a u32 overflow one step out of s first (u32 searches from its out-neighbours) instead of the u64 hash-map search: board 0.2068 s with S +21 (run-7d2fa7e4cfac, 61.60 points). Reverted to step 8 | run-7d2fa7e4cfac | 0.2068 | 282.6 | 61.60 |
| — | Per-search open-addressing tables again, this time on top of step 8 (bucketed reverse CSR, so step 5's double scan is not the cause): board 0.438 s, M 260 MB (run-4ae948dc808b, 53.80 points). The tables themselves are slow on the box (on the PC: 0.44 extra probes per lookup, no clustering). Reverted to step 8 | run-4ae948dc808b | 0.438 | 259.7 | 53.80 |
| 9 | S: unix-only `load`, a shorter `par()` (commit 1309be5) | run-17d166f4a76b | 0.2076 | 282.8 | 61.77 |
| 10 | Both halves' labels in one array of `[u32; 2]`: the meeting check at a settled node reads the cache line of its own label. PC neutral (commit f651ca9, pushed as f9bb30a: 0.2045 s; same code again at 39f5587: 0.2009 s) | run-4060dd92a5fd | 0.2009 | 285.9 | 61.98 |
| 11 | Less serial work: the leftover candidates found in parallel before the single-thread claim, the new offsets as a parallel prefix sum over per-thread degree chunks (commit 09a92cf, pushed as 2bf70a0) | run-9a0fbcc43821 | 0.1939 | 278.8 | 62.36 |
| — | The BFS orders copied into place by one thread each instead of one `concat`, the id map allocated without a fill pass (every slot is written): board 0.1950 s with S +14 (commit 8b7582b, pushed as 4496193, 62.18 points); reverted to step 11 | run-610877faa8d5 | 0.1950 | 278.9 | 62.18 |
| — | Paged labels: each thread keeps a pool of 4 KB pages (512 nodes of `[u32; 2]`) and a directory per 512 nodes instead of n labels; a search touches a few hundred pages. M 279 → 252 MB (so the peak is in the build, not the searches), T 0.2103 s (+6 % against the best): 61.62 points (commit 43b86ce) | run-9c36700b81eb | 0.2103 | 252.2 | 61.62 |
| 12 | Memory: the parts (edges bucketed by tail) are freed right after the first CSR, and the reverse CSR is built in the old edge array from atomic in-degree counts (ends as a parallel prefix sum, each edge placed at its head's end minus one; after the renumbering a head is near its tail, so threads rarely share a counter); paged labels from the step before. PC: peak 267 → 210 MB with the 92 MB file read (Windows), 117 → 115 ms. Board: M 279 → 139 MB, T 0.2136 s (commit 3343b2c) | run-783c571ed206 | 0.2136 | 138.8 | 63.92 |
| 13 | Queries taken in the order of their sources' new ids, so the searches that run at the same time work on nearby parts of the graph and share the L3 (commit eaa8061) | run-97a70a6e5921 | 0.2076 | 137.1 | 64.26 |
| 14 | The same logic written for fewer formatted lines (tuple structs for the labels and the search halves, one range helper, the per-thread loops through `par`). S 429 → 363 (commit 93bcecc) | run-da19dbb800c2 | 0.2128 | 137.6 | 64.51 |
| 15 | The claim map, the id map and the in-degree counters come zeroed from calloc (0 = unclaimed, else 1 + the thread) and the renumbered edge array is not filled: no 7 MB passes in one thread before the parallel work. PC: the claim map alone was 2.3 ms of one thread (commit ade2053) | run-d8c083ab0c7a | 0.2106 | 138.6 | 64.56 |
| 16 | One label lookup per relaxed edge (a missing page means an unset label, which the new distance improves anyway); prefetches three rows ahead in the renumbered copy (offsets, edges, new ids of the heads) and in the BFS (the offsets of a node when it is queued, its edges four pops ahead). PC 114 → 109 ms (commit 53b727e) | run-011b0ad3d598 | 0.2003 | 137.2 | 65.06 |
| — | Label pages of 128 nodes instead of 512: the per-thread pools 17.6 → 11.4 MB (PC count over 12 threads), board M 137.2 → 130.7 MB, T 0.2045 s, 65.04 points against 65.06 (commit 31d35cb); kept for the steps after it | run-0233f6f0d5d8 | 0.2045 | 130.7 | 65.04 |
| 17 | Packed graphs. The parsed edges are one `u64` each: tail minus its bucket start, head, weight (the rare weight that does not fit goes to a per-thread list of triples); the CSR edges are `head | weight << bits(n)` in a `u32`; at full scale n < 2^21 leaves 11 bits, and 99.93 % of the weights are below 2^11, so the rest are kept exact in a list sorted by edge position (a binary search on the rare escape). Label pages of 128 nodes. PC peak private memory (file mapped, 12 threads) 129 → 81 MB, time unchanged; board M 137 → 90 MB (commit 8732957) | run-a4daf0dfb2bb | 0.2005 | 89.8 | 66.37 |
| 18 | Labels reset by clearing their pages when a search ends (no lists of touched nodes, one push less per new label; PC 108.7 → 106.1 ms), `MADV_DONTNEED` on each parse thread's part of `graph.txt`, step 17 written for fewer lines (commits 19d3986, 71254f8) | run-adde3c10618f | 0.1802 | 86.4 | 67.93 |
| — | Reverse edges in the old CSR's edge array (warm pages) instead of fresh memory: T 0.1825 s, M 86.4 → 97.4 MB (the old offsets stayed alive with the partly moved struct); reverted (commit ba83278) | run-9d9dc164116a | 0.1825 | 97.4 | 67.28 |

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

## Phase profile on the board (run-8b83ab14d7d5, commit b26840d, step 12 code)

Phase times in µs since `main` in the name of an extra file in `/out` (the run fails on purpose; the distances
were checked on the PC). 12 threads: parse done 14,461, first CSR 41,504, BFS order 60,929, new ids and offsets
70,934, renumbered edges 91,263, reverse CSR 109,421, queries read 109,543, queries done 211,703, written 211,842;
T 216.9 ms. Construction ~109 ms (the first CSR alone 27 ms), queries ~102 ms.

## Phase profile on the board (run-d6579cbd7e77, commit 7fb4dfe, step 15 code)

Parse 15.4 ms; first CSR: counts done 21.5, edges placed 36.7, end 37.1; parts freed and claim map 40.5; BFS
ends (last thread) 63.1, leftovers 63.6; new ids, degrees, offsets 71.8; renumbered edges 105.7; reverse CSR 123.6;
queries 123.7–228.4 ms; T 234.6 ms (a slow push: unchanged track-A code ran 8 % slower too). The renumbered
copy (34 ms) and the BFS (23 ms) are random reads of rows and ids; step 16 prefetches them.

## Phase profile on the board (run-3ad3f6dd91d8, commit 5764074, step 18 code, a slow push)

Parse 23.1 ms, packed CSR 40.3, claim map 42.9, BFS 64.6, ids and offsets 75.7, renumbered edges 97.9,
reverse CSR 134.0 (36 ms: twice the step-15 figure, its edge array was fresh memory), queries 134.2–244.7 ms.
The unchanged track-A code ran 0.0409 s in the same push (0.0329 s the push before): a slow hour on the box.
