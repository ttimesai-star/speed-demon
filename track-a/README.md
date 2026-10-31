# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change was
measured before it was kept; `PERF.md` lists the ones that were not.

## Approach

Rust, standard library only, one file (`src/main.rs`, about 210 lines after rustfmt), statically linked.

1. **Input.** `requests.log` is mapped with `mmap` (declared through `extern "C"`, no crate) and cut into one
   chunk per hardware thread at line starts. When a thread has parsed its chunk it drops those pages of the
   mapping (`MADV_DONTNEED`), so the unmapping is done by twelve threads instead of by the process exit.
2. **Parse, one pass, all threads.** Each line is read with 8-byte little-endian loads:
   - the hour index comes straight from the day, month and hour digits (the spec fixes the 72-hour span);
   - the method is skipped with a two-byte perfect hash that gives its length;
   - the endpoint is scanned eight bytes at a time for the space (SWAR zero-byte test) and hashed on the way;
     the 64-bit hash identifies it in a lock-free table shared by all threads (insert by CAS), and a second
     array keeps the first occurrence for the output. No key bytes are compared per line: two of the 1,842
     templates share a 64-bit hash with a chance of about 1,842^2 / 2^65, below 10^-12;
   - the latency is parsed eight digits at once with the SWAR multiply-and-shift trick, with a slow path for
     the rare 9- and 10-digit values.
   Each line becomes one `u32` record in a per-thread, per-hour vector:
   `(slot | 5xx << 12 | big << 13) << 16 | latency & 0xFFFF`. The high 16 bits of the ~15 % of latencies
   above 65 535 µs go to a second, short vector. The vectors start near the mean hour's size and grow;
   `PR_SET_THP_DISABLE` keeps their reserve out of 2 MB pages (the box has THP on).
3. **Output, one hour at a time, all threads.** A worker takes the next hour, counts its records by slot,
   scatters the latencies into one flat array grouped by slot (counting sort), frees the hour's records, and
   for every endpoint in byte order computes the nearest-rank p50/p95/p99: `sort_unstable` for groups of up
   to 32, otherwise three `select_nth_unstable` calls from the bottom up (p50 on the whole group, p95 above
   it, p99 above that). Numbers are printed by hand. A finished hour's buffer is handed to whichever worker
   holds the writer lock, which writes every buffer that is next in hour order.

Determinism: the output does not depend on thread timing. Within a group the order of latencies can differ
between runs, but percentiles depend only on the multiset; groups and hours are always written in order.
Five runs give one hash. Checked against the published hashes (seed 1 sample, full and tiny), against the
reference run locally (seed 2 sample and full, seed 3 sample), the spec's worked example and an empty file.

Memory: about 29 MB peak on the board, most of it the per-hour records (4.3 bytes per line).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
