# Track C: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 14.44 | 367 | 0 |
| 1 | Rust: mmap, chunk starts from the parity of the quotes before them (one counting pass), one byte-level record parser per thread, names interned by their raw bytes between the quotes in a shared lock-free table, one atomic bitset of name ids per country | | | | |

## Correctness checks

Every step: seed 1 sample and full (published hashes), seed 2 sample and full, seed 3 sample and seed 1 tiny
(hashes of the reference run locally on the same generated data), and the spec's worked example byte for byte.
