# Track C: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 14.44 | 367 | 0 |
| 1 | Rust: mmap, chunk starts from the parity of the quotes before them (one counting pass), one byte-level record parser per thread, names interned by their raw bytes between the quotes in a shared lock-free table, one atomic bitset of name ids per country; `PR_SET_THP_DISABLE` so the sparse bitsets stay sparse (CI with THP=always: M 47.8 → 20.3 MB) (commit e0430a1) | run-6f21cf617bbf | 0.140 | 21.7 | 59.76 |
| 2 | `perf` on step 1: 23 % in libc (`bcmp` of each name against its first occurrence, a random spot in the 202 MB mapping), 17 % byte-by-byte field scans, 10 % the country lookup. Names are copied once into a compact arena and compared eight bytes at a time; the dense id lives in the table key (no second array, no wait for the inserting thread); word-wise name hash; SWAR search for `"` and `,`; country map `u8` instead of `u16`. Linux A/B 572.0 → 361.5 ms (−37 %). Then the quote count in 32-byte blocks with a `u8` sum (vectorises) and a 2 MB name table: 347.2 → 321.0 ms (−7.5 %) (commit 0309b99) | run-c09aeb31fe3d | 0.118 | 18.2 | 62.10 |
| 3 | Popcount only over the words that can hold a name id (the sparse bitsets are 31.6 MB of address space; reading them all faulted in zero pages) (commit f79fc4d) | run-b16c8423e37a | 0.109 | 18.3 | 62.92 |
| 4 | Country index from a 26^3 table (a code is a base-26 number, so number order is byte order) instead of sorting 200,000 codes: the regions phase was 16 ms of single-threaded work on the PC, before any parallel part. Linux A/B 299.2 → 280.5 ms (−6 %); the board gained more, its 12 threads wait on serial work (commit ca4391b) | run-53fbc2a217df | 0.0816 | 18.0 | 66.02 |
| 5 | SWAR number parse: eight digits at once (a digit has bit 4 set; `,` `.` CR LF have it clear), for the id, the region id, the amount and the regions file. PC 92–107 → 77.5 ms on full, 18.5 → 13.6 ms on sample (commit a990d8a) | run-5e2036e1e69a | 0.0739 | 17.7 | 67.11 |
| — | PGO (training on 1.8 MB of the seed-2 sample, CRLF kept): board 0.0821 s, no gain; reverted | run-742f1717b622 | 0.0821 | 17.6 | 66.06 |
| 6 | No quote-counting pass: chunk starts from the section 2.2 guarantee (an LF followed by a digit or a BOM byte ends a record). PC 77.5 → 76.3 ms; one read of 202 MB less on the board (commit 18cace0) | run-b60748652c6a | 0.0730 | 17.7 | 67.46 |
| 7 | Ablation on the PC: without the name lookup the parse is 13 ms faster (a third of it), without the country lookup no faster. Two-stage pipeline: each record's name slot is prefetched and the previous record's name is resolved and marked. PC 76.8 → 74.6 ms (commit fb752f6, pushed as 7dc1c88) | run-1c1fd2c854ad | 0.0669 | 17.7 | 68.23 |
| 8 | Three-stage pipeline: the name's id, then a prefetch of its word in the country bitset, the bit set one record later (PC 74.6 → 71.5 ms); `regions.csv` mapped and read in two passes instead of a 3.2 MB list of rows plus a 2.2 MB copy of the file (commits 22aea26, ee7955a) | run-16bc02fcc2c1 | 0.0626 | 12.1 | 70.45 |
| 9 | Prefetch of the country entry right after the region id is parsed, used after the amount (commit fc68bc4, pushed as eb74b43) | run-7d802fb0711b | 0.0624 | 11.9 | 70.56 |
| 10 | S: unix-only `load` (commit 1309be5) | run-8151737f38dd | 0.0625 | 12.5 | 70.36 |
| — | Byte search for `"` and `,` 32 bytes at a time with AVX2, SWAR for the tail: PC 71.5 → 69.8 ms, board 0.0634 s, no gain (run-61208674b19c); reverted. Board runs of unchanged code now spread by up to 8 % (0.0625–0.0678 s) | run-61208674b19c | 0.0634 | 11.9 | 70.30 |
| — | `regions.csv` in two parallel passes (one chunk of rows per thread) and the parse threads through a shared `par()`: board 0.0663 s with S +8 (run-ba02dfd218a5, 69.62 points), no visible gain within the spread; reverted to step 10 | run-ba02dfd218a5 | 0.0663 | 12.6 | 69.62 |
| — | `prefetchw` (ET0 hint, `+prfchw`) for the bitset word, so the line is owned before its RMW: board 0.0654 s, within the spread of unchanged code (commit 43b86ce) | run-dd7960d78aca | 0.0654 | 12.5 | 69.87 |
| 11 | Structure from 64-byte AVX2 masks: quote, comma and LF bits, a prefix XOR of the quote bits marks the quoted spans (a `""` pair flips twice), the separators are the commas and LFs outside them, four commas and then the LF per record, and the quote count of a field gives its `""` pairs; no byte-by-byte field scans. The name hash takes the length and the first 16 bytes (no loop), and `regions.csv` is parsed by all threads behind a barrier (codes present, then ranks into a 10^7-entry byte map, untouched pages cost nothing). PC single thread 153 → 127 ms. S 308 → 369 (commit b26840d) | run-c03fc1c7868e | 0.0560 | 12.8 | 70.87 |
| 12 | Name slots of 32 bytes: the key word, then the first 16 bytes of the name inline (a longer name keeps its further words in the arena), filled behind a busy marker so no reader compares a half-written slot; 2^17 slots. Nine lookups in ten read one cache line instead of a slot and then its arena words. PC slower (138 vs 130 ms on one thread, its 96 MB L3 holds everything), board faster (commit eaa8061) | run-20344632fbda | 0.0531 | 13.4 | 71.25 |
| 13 | The same logic written for fewer formatted lines: the aggregate as an `[i64; 7]` with one merge function, field lengths by arithmetic instead of `if`/`else`, one `zeroed` and one `pf` (prefetch) helper, the name table and arena in one allocation. S 368 → 288; PC timing unchanged (130.5 vs 130.1 ms on one thread) (commit 93bcecc) | run-5fb4ccfdd39e | 0.0547 | 13.5 | 71.71 |
| — | Without `PR_SET_THP_DISABLE` (do 4 KB pages on the tables cost the board TLB misses?): T 0.0540 s, M 13.5 → 14.6 MB, no gain; the prctl stays (commit ade2053) | run-90a7a65bf1ff | 0.0540 | 14.6 | 71.53 |
| 14 | Each thread drops its whole pages of the input mapping (`MADV_DONTNEED`) when its part is parsed: the unmapping runs in the 12 threads instead of in the process exit (~5.6 ms outside `main` in the step-9 profile) (commit 53b727e) | run-35f43149a29b | 0.0521 | 13.6 | 72.17 |
| — | One thread per core (6 instead of 12): T 0.0656 s, much slower; the hyperthreads hide the cache misses (commit 31d35cb) | run-8e8483bdd36c | 0.0656 | 12.1 | 70.13 |
| 15 | Bitset rows 2^20 bits plus eight cache lines apart: rows exactly 128 KB apart put the ~200 used lines of every country into the same ~200 of the 2,048 L2 sets (commit 19d3986, pushed as 71254f8) | run-2ce029eb3c87 | 0.0516 | 13.3 | 72.36 |
| 16 | The regions pass marks a country code present only if it is not marked yet: all twelve threads were storing into the same few bytes, 200,000 times (commit 5764074) | run-8fc820d684fb | 0.0509 | 13.6 | 72.37 |
| 17 | The first 16 name bytes are read once per record and carried through the pipeline: the hash and the compare share them (PC one thread 137.6 → 131.5 ms) (commit ba83278) | run-b2396887a6ba | 0.0484 | 13.6 | 72.90 |
| 18 | Statically linked (`+crt-static` in `.cargo/config.toml`, no line of code) (commit 25001e9) | run-4632b89c3375 | 0.0482 | 13.2 | 73.08 |
| 19 | The same logic written for fewer formatted lines (one-line imports, no long tuples or chains, `then` instead of `if`/`else`): S 297 → 263. Its first run took 0.0651 s in a slow push (the PC showed 2–4 %), the second 0.0481 s (commits 5c01b22, 652aa9f) | run-73829cd50880 | 0.0481 | 13.4 | 73.43 |
| 20 | Names identified by a 64-bit hash: an 8-byte slot holds 44 hash bits and the id, the slot index is 17 more bits, so two names count as equal only if all 61 bits match; with 100,000 names the chance that any two do is about 100,000^2 / 2^62, below 10^-8 per dataset (all eight local datasets match their reference hashes). The table is 1 MB instead of 4 MB of inline keys and no bytes are compared. PC one thread 142 → 120 ms, 8 threads 22.5 → 19.1 ms (commit 65a830e) | run-c21f5e3b4e84 | 0.0416 | 10.5 | 76.16 |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 sample and full, seed 3 sample and seed 1 tiny
(hashes of the reference run locally on the same generated data), and the spec's worked example byte for byte.

## Phase profile on the board (run-b4212a90d5b4, commit 3343b2c)

A profiling build of the step-9 code plus the arena prefetch (phase times in µs in the name of an extra file in
`/out`, so the run fails on purpose; the aggregates were correct on the PC). 12 threads: `regions.csv` done at
3,348 µs (serial), chunk starts 3,445 µs, threads started 3,635–4,151 µs, parse ends 54,503 / 54,968 / 55,488 µs,
aggregation 55,790 µs, written 55,834 µs; T 61.4 ms, so ~5.6 ms are process start and exit. The parse is 51 ms on
12 threads (612 thread-ms) where the PC needs 23 ms on 8 (184 thread-ms): the board pays 3.3x per thread, more
than in track A (2.2x), so the name table, arena, bitsets and country map (about 9 MB) cost cache misses there.

## Ablation on the board (run-138c8acedbb3, commit 7fb4dfe)

The step-13 code without setting the bitset bits (wrong on purpose, "hash mismatch"): T 0.0523 s, M 9.8 MB, in a
push where the unchanged track-A code ran 8 % slower than at night. The shared bitset costs roughly a tenth of T
and 3.7 MB of M, less than per-thread pair buffers (13 MB) would cost in M points; it stays.

## After step 20 (PC, one thread, seed 1 full)

Ablations on the step-20 code: all of it 117 ms; without any name work 68.5 ms; hashing the name and
prefetching its slot ~25–35 ms of the difference, the table lookup ~10 ms, the bitset ~8 ms. A 16-byte SSE
load with a sliding mask for the first 16 name bytes saved 1.4 %: not taken.

## Phase profile on the board (run-c315fdc459ef, commit e62df42, step 20 code)

12 threads: regions done 1.48 ms (parallel; it was 3.35 ms serial), parse ends 38.7 / 39.1 / 40.3 ms, output
written 40.9 ms; T 42.1 ms. The parse is now nine tenths of T; the threads finish up to 1.6 ms apart.
