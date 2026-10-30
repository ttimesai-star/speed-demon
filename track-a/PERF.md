# Track A: performance log

Every step from the reference to the current version. Board run IDs are added once the team shows on the
board (the Board App was not installed at the time of the first pushes, so those pushes have no board run).

Where the numbers come from:

- **CI-harness**: the organisers' `harness/run_submission.py`, unmodified, on a GitHub runner
  (2 vCPU AMD EPYC 7763, Docker with the cgroupfs driver, `--cpuset 0-1`), seed 1 full.
  T = mean of the middle three of five runs, M = highest `memory.peak`. Slower than the scoring box
  (2 threads instead of 12), so it ranks versions, it does not predict the board.
- **PC phases**: native Windows build on a Ryzen 7 9800X3D (8 threads), phase timers in a local copy
  of the code, median of 7 runs, file load excluded (Windows has no mmap path here).
- **A/B**: `.github/workflows/sd-ab.yml`, native Linux binaries of several commits, interleaved runs in one VM.

Every row passed the seed-1 full hash `495ca096…1cd3` and the seed-1 sample hash `ca6311bb…aac3`.

| Step | Commit | Change | Run ID | T (s) | M (MB) | Source |
|---|---|---|---|---|---|---|
| 0 | — | The reference, unchanged | | 32.05 | 228 | board calibration |
| 1 | b6dcccc | Rust: mmap, two parallel parse passes, atomic counts, atomic scatter into one flat latency array, three nested `select_nth_unstable` per group | — | 0.389 | 38.8 | CI-harness |
| 2 | bbffd6c | One parse pass into per-thread, per-hour (latency, slot) vectors; per-hour counting sort by slot; SWAR endpoint scan; method length from a two-byte perfect hash; ordered streaming write, records freed hour by hour | — | 0.164 | 23.2 | CI-harness |
| 3 | bfa3118 | SWAR latency parse (8 digits at once), word-wise key compare instead of `memcmp` | — | 0.194 | 23.2 | CI-harness; A/B: 192.5 ms vs 164.0 ms for step 2 — slower |
| 4 | (this) | Step 3 with the word-wise compare reverted (slice `==`, glibc `bcmp`): SWAR latency only | — | — | — | A/B: 157.3 ms vs 164.0 ms for step 2 |

## Measurements that decided the design

- PC phases, v2: parse 17.2 ms, per-hour output phase 10.5 ms (8 threads).
- Ablation (PC): skipping the endpoint key compare saves ~6 ms of the 17 ms parse; the compare is the
  most expensive step per line. Replacing `memcmp` with word compares saved under 1 ms, so the cost
  is the access to the key bytes, not the call.
- Ablation (PC): number formatting with `write!` costs ~1 ms, the file write ~0.5 ms; freeing the
  per-hour vectors ~3 ms on Windows (kept: it holds M down while the output grows).

## Failed or reverted

- **Word-wise key compare** (step 3): on Windows it looked neutral (−0.8 ms), on Linux it cost +22 %
  (A/B 192.5 vs 157.3 ms): glibc's `bcmp` beats a hand loop with per-word bounds checks. Reverted.
