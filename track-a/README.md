# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change was
measured before it was kept; `PERF.md` lists the ones that were not.

## Approach

Rust, standard library only, one file (`src/main.rs`, about 200 lines after rustfmt).

1. **Input.** `requests.log` is mapped with `mmap` (declared through `extern "C"`, no crate), so the
   input costs no heap and no copy. It is cut into one chunk per hardware thread at line starts.
2. **Parse, one pass, all threads.** Each line is read with 8-byte little-endian loads:
   - the hour index comes straight from the day, month and hour digits (the spec fixes the 72-hour span);
   - the method is skipped with a two-byte perfect hash that gives its length;
   - the endpoint is scanned eight bytes at a time for the space (SWAR zero-byte test) and hashed on the way;
     a lock-free open-addressing table shared by all threads maps it to a slot (insert by CAS; the key is the
     offset and length of the endpoint's first occurrence in the mapping, compared with `==`, which is glibc's
     `bcmp`);
   - the latency is parsed eight digits at once with the SWAR multiply-and-shift trick, with a slow path for
     the rare 9- and 10-digit values.
   Each line becomes one `u32` record in a per-thread, per-hour vector:
   `(slot | 5xx << 12 | big << 13) << 16 | latency & 0xFFFF`. The high 16 bits of the ~15 % of latencies
   above 65 535 µs go to a second, short vector. The vectors are reserved for the worst case (a line has at
   least 40 bytes), so they never grow; untouched reserve costs no memory.
3. **Output, one hour at a time, all threads.** A worker takes the next hour, counts its records by slot,
   scatters the latencies into one flat array grouped by slot (counting sort), frees the hour's records, and
   for every endpoint in byte order computes the nearest-rank p50/p95/p99: `sort_unstable` for groups of up
   to 32, otherwise three nested `select_nth_unstable` calls (p99 on the whole group, p95 below it, p50 below
   that). Numbers are printed by hand. A finished hour's buffer is handed to whichever worker holds the writer
   lock, which writes every buffer that is next in hour order, so no worker waits for the hour before it.

Determinism: the output does not depend on thread timing. Within a group the order of latencies can differ
between runs, but percentiles depend only on the multiset; groups and hours are always written in order.
Five runs give one hash. Checked against the reference on seed 1 (sample and full, the published hashes)
and seed 2 (sample and full, hashes of the reference run locally).

Memory: about 19 MB peak, measured with the organisers' harness. 4.6 MB of it is the process itself (mmap and
exit only), about 13 MB the records, the rest the output page cache and buffers.

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

- Profile on the scoring box itself: the runner I measured on has 2 vCPUs, the box has 12 threads, and the
  balance between the parse and the output phase will differ.
- PGO is measured (−2 %) and ready; it would go in last, because the profile follows the code.
- The endpoint compare (`bcmp`) is the largest single cost left in the parse. An exact compare that avoids
  touching the key bytes would need the key words inline in the hash table entry.
- Records at 4.3 bytes per line are most of M; a variable-length latency encoding would bring them to
  about 3.5 bytes at some cost in time.
