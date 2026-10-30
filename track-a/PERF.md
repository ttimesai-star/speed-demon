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
| 7 | 840f966 | `word()`: one length check and an unaligned load instead of `d.get(p..p + 8)` + `try_into` (the range checks showed up as 6–7 % in `perf`) | — | — | — | A/B: 116.9 ms vs 122.2 ms for step 6 (−4.3 %) |
| 8 | 5bf7389 | Compact rewrite for S: same algorithm, mmap-only load (the board is Linux), helpers for the SWAR masks, recursive digit printer, one closure per output worker. S 255 → 196 lines after rustfmt | — | 0.146 | 19.4 | CI-harness; A/B: 147.9 vs 147.2 ms for step 7 (neutral) |
| 9 | 2d1fe2d | One `u32` record per line, `(slot \| 5xx << 12 \| big << 13) << 16 \| latency & 0xFFFF`, instead of two `u16` vectors: one push per line instead of two | — | — | — | A/B: 103.4 vs 106.0 ms for step 8 (−2.5 %) |
| 10 | 3170785 | Hour index from one 8-byte load of `MM-DDTHH` (low nibbles) instead of five bounds-checked byte loads | — | — | — | A/B: 116.6 vs 117.8 ms for step 9 (−1 %) |
| 11 | 806d0d8 | PGO: instrumented build, training run on a 60,000-line slice of the seed-2 sample (`pgo/`, data, not code; seed 2 so the profile is not fitted to the board's seed), `llvm-profdata merge`, rebuild, all inside the same seven Dockerfile lines, from the allowlisted `halloumee/speed-demon-rust:1.99.1-trixie-llvm-tools` | board push 16:20 UTC, team not on the board yet | 0.144 | 18.5 | A/B (Docker-built): 114.8 vs 117.8 ms for step 9 (−2.5 %); CI-harness on a different CPU (EPYC 9V74) |
| 12 | 515ff24 | Iterative digit printer that writes the leading space with the digits in one `extend` (the recursive one was 4.7 % in `perf`, not inlined); unstable sort for the endpoint order | board push 16:35 UTC, team not on the board yet | 0.137 | 18.5 | A/B (Docker-built): 136.0 vs 138.3 ms for step 11 (−1.6 %); CI-harness on EPYC 7763, the CPU of steps 1–2 |
| 12b | a350a88 | Step 12, first scored board run | run-299ca5c2b1ad | 0.132 | 730.7 | board: K 10.14, 50.30 points (23rd of 33) |
| 13 | 4296630 | Per-hour vectors reserved near the mean hour (`(b - a) / 4096` records) instead of the worst case (`(b - a) / 40`), and `prctl(PR_SET_THP_DISABLE)` at start. The board box backs the 12 × 72 worst-case reservations with transparent huge pages: M was 730 MB there and 18.5 MB on our runner, and zeroing those pages cost most of T. CI with THP=always and 12 threads forced: 55.5 → 30.1 MB | run-ffe31d35df4b | 0.0340 | 29.8 | board: K 37.80, 78.87 points (1st of 37) |

## Measurements that decided the design

- PC phases, step 2: parse 17.2 ms, per-hour output phase 10.5 ms (8 threads).
- Ablation (PC): skipping the endpoint key compare saves ~6 ms of the 17 ms parse; the compare is the
  most expensive step per line. Replacing `memcmp` with word compares saved under 1 ms, so the cost
  is the access to the key bytes, not the call.
- Ablation (PC): number formatting with `write!` costs ~1 ms, the file write ~0.5 ms; freeing the
  per-hour vectors ~3 ms on Windows (kept: it holds M down while the output grows).

## Linux `perf` profile (step 6, 2 threads)

`perf record` on the runner (no hardware counters in the VM, so task-clock sampling): our code 73 %, libc 16 %
(about 11 % of it `bcmp` from the endpoint compare, about 3 % `memcpy` from output assembly), kernel 11.5 %
(file-mapping faults, zeroing fresh anonymous pages, memcg charging, teardown at exit). Inside our code the
endpoint scan loop and the `word()` range checks led; `select_nth_unstable` was 7 %.

Memory floor, measured with the harness on cut-down builds: mmap + exit only = 4.6 MB; after the parse,
before any output = 18.3 MB; full run = 19.2 MB. The records (4.3 bytes per line, 13.3 MB) are nearly all
of M above the floor.

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
- **Inline word-wise compare again, on top of the faster `word()`** (exp-cmp2; A/B 112.0 vs 95.8 ms, +17 %):
  glibc `bcmp` still wins. Reverted.
- **`madvise(MADV_POPULATE_READ)` on each thread's chunk** (exp-madv; A/B 98.3 vs 95.8 ms, +2.6 %):
  populating the page tables up front costs more than the fault-around path. Not taken.
- **Endpoint hash from the length, the first 16 bytes and the last 8**, computed after the scan so the scan
  loop has no multiply chain (exp-hash3; A/B 122.8 vs 116.6 ms, +5 %): more probes and the extra loads cost
  more than the shorter loop saved. Not taken.

- **First compact rewrite** (exp-compact, 9ef8b93; A/B 162.8 vs 148.2 ms, +10 %). Bisected with three
  variants that each put one old piece back: the culprit was folding the endpoint-scan exit into the loop body
  with a `checked_shl` mask on every word; the recursive digit printer and indexed high-half reads were neutral.
  Step 8 is the compact version with the old scan loop.

- **Insertion sort threshold 16** for small groups (exp-sort16): aborts. At n = 17 the p95 and p99 ranks are
  equal and the nested `select_nth_unstable` gets an index equal to its slice length. The ranks are distinct
  from n = 25 on, so the threshold stays at 32 (now commented in the code). **Threshold 64**: 117.2 vs
  117.8 ms, noise. Not taken.

## Correctness checks (current version)

Seed 1 sample and full (published hashes), seed 2 sample `fd26778f…` and full `fe3469ff…` (reference run
locally on the same generated data), seed 1 tiny (published), the spec's worked example, an empty file.
Every A/B binary is run 15–21 times and each run's hash is recorded: one hash per binary every time.

## After step 13 (tried on the board, reverted)

- **Worst-case per-hour reserve again** (`(b - a) / 40` records, THP still off): harness with 12 threads forced
  28.7 vs 29.8 MB, but the board ran it in 0.0440 s instead of 0.0333 s (run-5f9529320ff1, 75.99 points):
  1,728 separate large reservations cost more there than the small growing vectors. Reverted to step 13.
- **`malloc_trim(0)` after every sixth hour of output**, so that freed records leave M while the output file grows:
  M 29.8 → 25.8 MB but T 0.0350 s (run-08f3b828211b, 79.12 points against 79.27). Reverted.
