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

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 sample and full, seed 3 sample and seed 1 tiny
(hashes of the reference run locally on the same generated data), and the spec's worked example byte for byte.
