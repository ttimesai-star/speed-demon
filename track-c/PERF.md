# Track C: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 14.44 | 367 | 0 |
| 1 | Rust: mmap, chunk starts from the parity of the quotes before them (one counting pass), one byte-level record parser per thread, names interned by their raw bytes between the quotes in a shared lock-free table, one atomic bitset of name ids per country; `PR_SET_THP_DISABLE` so the sparse bitsets stay sparse (CI with THP=always: M 47.8 → 20.3 MB) (commit e0430a1) | run-6f21cf617bbf | 0.140 | 21.7 | 59.76 |
| 2 | `perf` on step 1: 23 % in libc (`bcmp` of each name against its first occurrence, a random spot in the 202 MB mapping), 17 % byte-by-byte field scans, 10 % the country lookup. Names are copied once into a compact arena and compared eight bytes at a time; the dense id lives in the table key (no second array, no wait for the inserting thread); word-wise name hash; SWAR search for `"` and `,`; country map `u8` instead of `u16`. Linux A/B 572.0 → 361.5 ms (−37 %). Then the quote count in 32-byte blocks with a `u8` sum (vectorises) and a 2 MB name table: 347.2 → 321.0 ms (−7.5 %) | | | | |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 sample and full, seed 3 sample and seed 1 tiny
(hashes of the reference run locally on the same generated data), and the spec's worked example byte for byte.
