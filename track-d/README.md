# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`), statically linked.

1. **Input.** `graph.txt` is mapped with `mmap` and cut into one chunk per hardware thread at line starts.
   Numbers are parsed eight digits at a time (SWAR). Each parsed edge is one `u64` (tail minus its bucket
   start, head, weight) in a per-thread bucket by tail range; the rare weight that does not fit goes to a
   short list of triples. Self-loops are dropped. Each thread drops its pages of the mapping when done.
2. **Packed CSR, no atomics.** Thread k counts, sums and fills the CSR of tail range k. An edge is one `u32`:
   `head | weight << bits(n)`; at full scale that leaves 11 bits, and 99.93 % of the weights are below 2^11,
   so the others keep the field's top value and their exact weight sits in a list sorted by edge position.
3. **Renumbering for locality.** Node ids are a random relabelling, so a search would jump all over memory.
   Each thread runs a BFS from its own seed and claims nodes by CAS (claims, ids and counters come from
   zeroed memory: no pass over them in one thread); the BFS orders give the new ids and the CSR is copied in
   that order, with prefetches several rows ahead.
4. **Reverse CSR** for the backward search: in-degrees counted with atomic increments in one flat pass,
   their ends as a parallel prefix sum, each edge placed at its head's end minus one. After the renumbering
   a head is near its tail, so threads rarely share a counter.
5. **Queries, all threads.** Queries are taken in the order of their source's new id, so the searches that
   run together share the cache. `s == t` is 0; an endpoint with no edges on the needed side is `-1`. Every
   other query is a bidirectional Dijkstra search with radix heaps, settling the side with the smaller top,
   stopping when the two tops sum to at least the best meeting (checked when a node is settled). Labels are
   `u32` pairs (forward, backward) in pages of 128 nodes from a per-thread pool, so a thread holds a few MB
   instead of n labels; a search clears its pages when it ends. A query whose sum reaches 2^32 - 1 (the
   weights near 2^32 on the special nodes) is redone with `u64` distances in a hash map.
6. **Output.** Answers are stored by query index and written in input order, so the output does not depend
   on which worker answered which query.

Memory peaks at about 85 MB on the board (279 MB before the paged labels and the packed edges).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md` for what is planned next.
