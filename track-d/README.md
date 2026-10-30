# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. **Input.** `graph.txt` is mapped with `mmap` (declared through `extern "C"`, no crate) and cut into one
   chunk per hardware thread at line starts. Numbers are parsed eight digits at a time (SWAR).
2. **Graph without atomics.** Each thread sorts its edges into buckets by tail range; thread k then counts,
   sums and fills the CSR of range k. Self-loops are dropped (they never shorten a path); parallel edges and
   duplicate lines are kept, Dijkstra takes the lighter one by itself. (A first version placed edges with an
   atomic cursor per node: those locked increments were 28 % of the profile.)
3. **Renumbering for locality.** Node ids are a random relabelling, so a search jumps all over memory. Each
   thread runs a BFS from its own seed and claims nodes with a compare-and-swap; a sweep (a plain load before
   any CAS) picks up the nodes no BFS reached. The BFS orders give the new ids, and the CSR is rebuilt in that
   order in parallel. Neighbours now sit close together in the edge and label arrays.
4. **Reverse CSR** for the backward search, built the same way, in the forward pass's buckets and the old
   edge array, so it costs no fresh pages.
5. **Queries, all threads.** Workers take query indices from an atomic counter. `s == t` is 0; a target with
   no in-edges or a source with no out-edges is `-1` at once (these are the queries that make the reference
   search the whole graph). Every other query is a bidirectional Dijkstra search (binary heaps of
   `dist << 32 | node`), each half settling the side with the smaller top key, stopping when the two tops
   sum to at least the best meeting. The meeting is checked when a node is settled: on every edge of a
   shortest path, the second of its two ends to be settled sees the other half's label. Labels are `u32`;
   a query whose sum reaches 2^32 - 1 (the weights near 2^32 on the special nodes) is redone with `u64`
   distances in a hash map. Each worker keeps its label arrays for all its queries and resets only the
   entries it touched.
6. **Output.** Answers are stored by query index and written in input order, so the output does not depend on
   which worker answered which query.

Tried and dropped (numbers in `PERF.md`): radix and 4-ary heaps, per-search hash tables instead of dense
label arrays (half the memory, twice the time on the board), PGO, zeroed label arrays, prefetching.

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md` for what is planned next.
