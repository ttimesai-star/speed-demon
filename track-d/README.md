# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. **Input.** `graph.txt` is mapped with `mmap` (declared through `extern "C"`, no crate) and cut into one
   chunk per hardware thread at line starts.
2. **Graph, two parallel passes.** Pass 1 counts out-degrees and in-degrees with atomic counters; a prefix
   sum gives the CSR offsets. Pass 2 parses the lines again and drops each edge `(head, weight)` at an atomic
   cursor of its tail. Self-loops are dropped (they never shorten a path). Parallel edges and duplicate lines
   are kept: Dijkstra takes the lighter one by itself.
3. **Queries, all threads.** Workers take query indices from an atomic counter. `s == t` is 0; a target with
   no in-edges or a source with no out-edges is `-1` at once (these are the queries that make the reference
   search the whole graph). Every other query is a Dijkstra search from `s` with a binary heap that stops when
   `t` is settled. Each worker keeps one distance array for all its queries and resets only the entries it
   touched. Distances are `u64`: sums go above 2^32.
4. **Output.** Answers are stored by query index and written in input order, so the output does not depend on
   which worker answered which query.

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md` for what is planned next.
