# Track D: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 72.69 | 893 | 0 |
| 1 | Rust: mmap, two-pass parallel CSR, per-query Dijkstra (binary heap, early stop) on all threads, `-1` at once for targets with no in-edges and sources with no out-edges (commit 4296630) | run-fb3879b67c30 | 0.909 | 234.8 | 49.16 |
| 2 | Parallel BFS renumbering (one BFS per thread, CAS claims), CSR rebuilt in the new order; `u32` distances and one-word heap entries, `u64` hash-map fallback for sums ≥ 2^32 − 1. PC (8 threads): queries 345 → 130 ms, whole run ~400 → ~220 ms | | | | |

## PC measurements (Ryzen 7 9800X3D, 8 threads, Windows build, `fs::read` instead of mmap)

| Variant | Parse + CSR | Renumber | Queries | Total |
|---|---|---|---|---|
| Step 1 | 55 ms | — | 345 ms | ~400 ms |
| + single-thread BFS renumbering | 55 ms | 63 ms | 225 ms | ~350 ms |
| + bidirectional Dijkstra (reverse CSR) | 55 ms | 63 ms + 11 ms | 230–265 ms | ~400 ms — not taken |
| + parallel BFS (step 2 without u32) | 55 ms | 35 ms | 200 ms | ~300 ms |
| + `u32` distances (step 2) | 47 ms | 28 ms | 130 ms | ~210–250 ms |
| + radix heap | 47 ms | 28 ms | 140 ms | 267 vs 255 ms median of 7 — noise, not taken |

The board runs ~2.2x slower than the PC on step 1 (0.909 s vs ~0.4 s): the board's i5-13500 has 24 MB of L3
against the PC's 96 MB, and Dijkstra on randomly labelled nodes is bound by cache misses. That is why
locality (step 2) came first.

Correctness, every step: seed 1 sample and full (published hashes), seed 2 sample and full and seed 1 tiny
(reference run locally on the same data), the spec's worked example.
