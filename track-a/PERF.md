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
| 4 | 004e829 | Step 3 with the word-wise compare reverted (slice `==`, glibc `bcmp`): SWAR latency only | — | — | — | A/B: 157.3 ms vs 164.0 ms for step 2 |
| 5 | 5e8b926 | Hand-written integer formatting instead of `write!`; `sort_unstable` for groups of ≤ 32 instead of three selects; finished hour buffers are handed to whichever thread holds the writer lock, so no thread waits for the hour before it | — | — | — | A/B: 145.4 ms vs 155.4 ms for step 4 (itoa alone: 145.2) |
| 6 | d2fdcf7 | Latency stored as low 16 bits per line plus high 16 bits only for the ~15 % of lines above 65 535 µs (4.3 instead of 6 bytes per line) | — | — | 19.2 | A/B: 148.2 ms vs 145.4 ms for step 5 (+2 % T, −17 % M: K up ~2.8 %) |

## Measurements that decided the design

- PC phases, v2: parse 17.2 ms, per-hour output phase 10.5 ms (8 threads).
- Ablation (PC): skipping the endpoint key compare saves ~6 ms of the 17 ms parse; the compare is the
  most expensive step per line. Replacing `memcmp` with word compares saved under 1 ms, so the cost
  is the access to the key bytes, not the call.
- Ablation (PC): number formatting with `write!` costs ~1 ms, the file write ~0.5 ms; freeing the
  per-hour vectors ~3 ms on Windows (kept: it holds M down while the output grows).

## Linux phase profile (step 4, 2 threads, A/B VM)

Load ~0.01 ms (mmap), parse ~98 ms, regroup ~0.5 ms, then per output worker: counting 2.8 ms, scatter 3.2 ms,
freeing records 3.5 ms, select + format 32 ms, waiting for the previous hour 0–3 ms, write 2.5 ms.
Formatting was the largest single piece of "select + format", hence step 5.

## Failed or reverted

- **Word-wise key compare** (step 3): on Windows it looked neutral (−0.8 ms), on Linux it cost +22 %
  (A/B 192.5 vs 157.3 ms): glibc's `bcmp` beats a hand loop with per-word bounds checks. Reverted.
- **`mem::forget` instead of freeing each hour's records** (A/B 155.4 vs 156.8 ms, −1 %): not worth the
  memory, because the output page cache then stacks on top of the records. Not taken.
- **`MAP_POPULATE`** on the input mapping (A/B 168.2 vs 156.8 ms, +7 %): one thread faults the whole
  239 MB instead of twelve threads faulting their own chunks. Not taken.
- **Rotate-xor per word instead of a multiply in the endpoint hash** (A/B 155.8 vs 156.8 ms): within noise. Not taken.
- **Thread-local endpoint tables with a compact key arena** and dense global ranks (exp-local, 697f8ea;
  A/B 162.9 vs 151.2 ms, +8 %): the idea was that keys scattered over the 239 MB mapping cost TLB misses.
  The extra rank lookup per record and per-thread insertion cost more than they saved. Reverted.
